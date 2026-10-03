//! 原子保存自建歌单的有序歌曲身份，读取时关联共享的当前元数据。

use super::super::entity::{user_playlist_entries, user_playlists};
use super::super::{Error, Result, ServerStore};
use mineral_model::{
    CollectionIndex, Playlist, PlaylistActions, PlaylistEntry, PlaylistId, Song, SongId, SourceKind,
};
use rustc_hash::FxHashMap;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait};

impl ServerStore {
    /// 一次事务保存完整队列的身份与顺序，并补入缺失的共享元数据。
    /// 每次生成独立的歌单身份，已存在的歌曲资料不被队列覆盖。
    pub(crate) async fn create_user_playlist(
        &self,
        name: &str,
        songs: &[Song],
    ) -> Result<PlaylistId> {
        let db = self.pool().ok_or(Error::Disabled)?;
        let id = PlaylistId::new(SourceKind::MINERAL, uuid::Uuid::new_v4().to_string());
        let entries = songs
            .iter()
            .enumerate()
            .map(|(position, song)| {
                Ok(user_playlist_entries::ActiveModel {
                    playlist_id: Set(id.value().to_owned()),
                    position: Set(i64::try_from(position)?),
                    song_namespace: Set(song.source().name().to_owned()),
                    song_value: Set(song.id.value().to_owned()),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut by_source = FxHashMap::<SourceKind, Vec<&Song>>::default();
        for song in songs {
            by_source.entry(song.source()).or_default().push(song);
        }
        let tx = db.begin().await.map_err(database_error)?;
        for (source, songs) in by_source {
            self.scope(source)
                .insert_missing_meta_batch_on(&tx, &songs)
                .await?;
        }
        user_playlists::Entity::insert(user_playlists::ActiveModel {
            id: Set(id.value().to_owned()),
            name: Set(name.to_owned()),
            track_count: Set(i64::try_from(songs.len())?),
        })
        .exec(&tx)
        .await
        .map_err(database_error)?;
        for batch in entries.chunks(100) {
            user_playlist_entries::Entity::insert_many(batch.to_vec())
                .exec_without_returning(&tx)
                .await
                .map_err(database_error)?;
        }
        tx.commit().await.map_err(database_error)?;
        mineral_log::info!(target: "persist", playlist_id = %id, entries = songs.len(), "saved user playlist song identities");
        Ok(id)
    }

    /// 列出自建歌单头部，不加载歌曲资料。
    pub(crate) async fn user_playlists(&self) -> Result<Vec<Playlist>> {
        let Some(db) = self.pool() else {
            return Ok(Vec::new());
        };
        user_playlists::Entity::find()
            .order_by_asc(user_playlists::Column::Id)
            .all(db)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(playlist_header)
            .collect()
    }

    /// 按保存的位置关联当前歌曲资料，保留来源、原始位置与重复项。
    /// 暂无元数据的条目不返回，但不删除其关系，资料恢复后可再次读取。
    pub(crate) async fn user_playlist(&self, id: &PlaylistId) -> Result<Option<Playlist>> {
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        if id.namespace() != SourceKind::MINERAL {
            return Ok(None);
        }
        let tx = db.begin().await.map_err(database_error)?;
        let Some(row) = user_playlists::Entity::find_by_id(id.value())
            .one(&tx)
            .await
            .map_err(database_error)?
        else {
            return Ok(None);
        };
        let rows = user_playlist_entries::Entity::find()
            .filter(user_playlist_entries::Column::PlaylistId.eq(id.value()))
            .order_by_asc(user_playlist_entries::Column::Position)
            .all(&tx)
            .await
            .map_err(database_error)?;
        let mut by_source = FxHashMap::<SourceKind, Vec<SongId>>::default();
        let mut relations = Vec::with_capacity(rows.len());
        for entry in rows {
            let source = SourceKind::from_name(&entry.song_namespace);
            let song_id = SongId::new(source, entry.song_value);
            by_source.entry(source).or_default().push(song_id.clone());
            relations.push((
                CollectionIndex::new(u64::try_from(entry.position)?),
                song_id,
            ));
        }
        let mut metadata = FxHashMap::default();
        for (source, ids) in by_source {
            metadata.extend(self.scope(source).get_meta_batch_on(&tx, &ids).await?);
        }
        tx.commit().await.map_err(database_error)?;
        let mut playlist = playlist_header(row)?;
        for (index, song_id) in &relations {
            if let Some(song) = metadata.get(song_id) {
                playlist.entries.push(
                    PlaylistEntry::builder()
                        .index(*index)
                        .song(song.clone())
                        .build(),
                );
            }
        }
        let missing = relations.len() - playlist.entries.len();
        if missing > 0 {
            mineral_log::warn!(target: "persist", playlist_id = %id, missing, "user playlist entries have no readable song metadata");
        }
        mineral_log::debug!(target: "persist", playlist_id = %id, entries = relations.len(), loaded = playlist.entries.len(), "loaded user playlist with current song metadata");
        Ok(Some(playlist))
    }

    /// 改名保留曲目和身份。
    pub(crate) async fn rename_user_playlist(&self, id: &PlaylistId, name: &str) -> Result<()> {
        let db = self.pool().ok_or(Error::Disabled)?;
        let result = user_playlists::Entity::update_many()
            .col_expr(
                user_playlists::Column::Name,
                sea_orm::sea_query::Expr::value(name),
            )
            .filter(user_playlists::Column::Id.eq(id.value()))
            .exec(db)
            .await
            .map_err(database_error)?;
        if result.rows_affected == 0 {
            return Err(Error::PlaylistNotFound { id: id.clone() });
        }
        Ok(())
    }

    /// 删除歌单与其条目，不接触歌曲、收藏或播放队列。
    pub(crate) async fn delete_user_playlist(&self, id: &PlaylistId) -> Result<()> {
        let db = self.pool().ok_or(Error::Disabled)?;
        let result = user_playlists::Entity::delete_by_id(id.value())
            .exec(db)
            .await
            .map_err(database_error)?;
        if result.rows_affected == 0 {
            return Err(Error::PlaylistNotFound { id: id.clone() });
        }
        Ok(())
    }
}

/// 歌单头部携带本地可管理的操作资格。
fn playlist_header(row: user_playlists::Model) -> Result<Playlist> {
    Ok(Playlist::builder()
        .id(PlaylistId::new(SourceKind::MINERAL, row.id))
        .name(row.name)
        .track_count(u64::try_from(row.track_count)?)
        .actions(PlaylistActions {
            rename: true,
            delete: true,
        })
        .build())
}

/// 保存 SQLite 原始错误链。
fn database_error(source: sea_orm::DbErr) -> Error {
    Error::Database {
        operation: "访问自建歌单",
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Error, PlaylistId, ServerStore, SourceKind, user_playlist_entries, user_playlists,
    };
    use crate::persistence::entity::{song_artists, song_meta};
    use mineral_model::{Song, SongId};
    use sea_orm::EntityTrait;
    use sea_orm::{ConnectionTrait, PaginatorTrait};

    /// 后续批次失败不得留下半张歌单。
    #[tokio::test]
    async fn entry_failure_rolls_back_header_and_all_previous_entries() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("test.db")).await?;
        let db = store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("missing database"))?;
        db.execute_unprepared("CREATE TRIGGER reject_second_batch BEFORE INSERT ON user_playlist_entries WHEN NEW.position = 100 BEGIN SELECT RAISE(ABORT, 'injected entry failure'); END;").await?;
        let songs = (0..101)
            .map(|i| {
                let mut song = mineral_test::song(&i.to_string());
                song.artists.push(mineral_model::ArtistRef {
                    id: mineral_model::ArtistId::new(song.source(), "artist"),
                    name: "Artist".to_owned(),
                });
                song
            })
            .collect::<Vec<_>>();
        assert!(
            store
                .create_user_playlist("rollback", &songs)
                .await
                .is_err()
        );
        assert_eq!(user_playlists::Entity::find().count(db).await?, 0);
        assert_eq!(user_playlist_entries::Entity::find().count(db).await?, 0);
        assert_eq!(song_meta::Entity::find().count(db).await?, 0);
        assert_eq!(song_artists::Entity::find().count(db).await?, 0);
        Ok(())
    }

    /// 来源资料暂时消失时保留成员身份与位置，重建后重新可见。
    #[tokio::test]
    async fn song_relations_survive_cache_clear_and_source_replacement() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test.db");
        let store = ServerStore::open(&path).await?;
        let song = mineral_test::song("persisted");
        let other = Song::builder()
            .id(SongId::new(SourceKind::BILIBILI, song.id.value()))
            .name("other source".to_owned())
            .build();
        let songs = vec![song.clone(), other.clone(), song.clone()];
        let id = store.create_user_playlist("relations", &songs).await?;
        store.clear_playlist_caches().await?;
        store.scope(song.source()).replace_meta_batch(&[]).await?;
        let partial = store
            .user_playlist(&id)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("playlist missing"))?;
        assert_eq!(partial.track_count, 3);
        assert_eq!(partial.entries.len(), 1);
        let entry = partial
            .entries
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("entry missing"))?;
        assert_eq!(entry.index.get(), 1);
        assert_eq!(entry.song, other);
        store
            .scope(song.source())
            .replace_meta_batch(&[&song])
            .await?;
        drop(store);
        let store = ServerStore::open(&path).await?;
        let playlist = store
            .user_playlist(&id)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("playlist missing"))?;
        assert_eq!(
            playlist
                .entries
                .into_iter()
                .map(|e| e.song)
                .collect::<Vec<_>>(),
            songs
        );
        store.delete_user_playlist(&id).await?;
        let db = store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("missing database"))?;
        assert_eq!(user_playlist_entries::Entity::find().count(db).await?, 0);
        Ok(())
    }

    /// 显式保存不沿用禁用缓存的静默成功语义。
    #[tokio::test]
    async fn disabled_store_rejects_explicit_playlist_writes() {
        let store = ServerStore::disabled();
        let id = PlaylistId::new(SourceKind::MINERAL, "saved");
        assert!(matches!(
            store
                .create_user_playlist("name", &[mineral_test::song("1")])
                .await,
            Err(Error::Disabled)
        ));
        assert!(matches!(
            store.rename_user_playlist(&id, "name").await,
            Err(Error::Disabled)
        ));
        assert!(matches!(
            store.delete_user_playlist(&id).await,
            Err(Error::Disabled)
        ));
    }
}
