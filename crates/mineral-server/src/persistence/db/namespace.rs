//! 某来源命名空间下的存储视图。

use mineral_log::trace;
use mineral_model::{AlbumId, ArtistId, CollectionIndex, PlaylistId, SongId, SourceKind};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};

use crate::persistence::ServerStore;
use crate::persistence::entity::{
    playlist_cache, playlist_entries, song_artists, song_favorites, song_meta,
};
use mineral_channel_core::store::{CachedPlaylistEntry, PlaylistCacheEntry};

/// 绑定单一来源 namespace 的结构态视图。降级 ServerStore 下所有方法 no-op/空。
pub struct NamespaceStore {
    /// 顶层句柄(经 `persist.pool()` 取连接池)。
    persist: ServerStore,

    /// 本视图绑定的来源(用 `source.name()` 做 namespace 过滤)。
    source: SourceKind,
}

impl NamespaceStore {
    /// 构造。
    ///
    /// # Params:
    ///   - `persist`: 顶层句柄
    ///   - `source`: 绑定的来源
    pub(crate) fn new(persist: ServerStore, source: SourceKind) -> Self {
        Self { persist, source }
    }

    /// Returns the immutable source bound to this store.
    pub fn source(&self) -> SourceKind {
        self.source
    }

    /// Rejects foreign owner identities before reads, writes or disabled-store returns.
    pub(crate) fn check_source(&self, actual: SourceKind) -> crate::persistence::Result<()> {
        if actual != self.source {
            mineral_log::warn!(target: "persist", expected = self.source.name(), actual = actual.name(), "rejected foreign source identity");
            return Err(crate::persistence::Error::NamespaceMismatch {
                expected: self.source,
                actual,
            });
        }
        Ok(())
    }

    /// Checks all identities that metadata storage writes under this source.
    pub(crate) fn check_song(&self, song: &mineral_model::Song) -> crate::persistence::Result<()> {
        self.check_source(song.source())?;
        if let Some(album) = &song.album {
            self.check_source(album.id.namespace())?;
        }
        for artist in &song.artists {
            self.check_source(artist.id.namespace())?;
        }
        Ok(())
    }

    /// 底层连接池(降级 ServerStore 为 `None`;同 crate 扩展方法用,如 song_kv)。
    pub(crate) fn pool(&self) -> Option<&sea_orm::DatabaseConnection> {
        self.persist.pool()
    }

    /// 本视图的 namespace 过滤值(= `source.name()`)。
    pub(crate) fn namespace(&self) -> &str {
        self.source.name()
    }

    /// 按 album id 回查专辑名(取任一成员歌 `song_meta.album_name`;专辑名只作为歌的投影存,
    /// 无独立专辑表)。降级 / 未命中返回 `Ok(None)`。
    ///
    /// # Params:
    ///   - `id`: 专辑 id(裸值查 `song_meta.album_id`)
    ///
    /// # Return:
    ///   命中返回 `Ok(Some(name))`,否则 `Ok(None)`。
    pub async fn album_name(&self, id: &AlbumId) -> crate::persistence::Result<Option<String>> {
        self.check_source(id.namespace())?;
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        song_meta::Entity::find()
            .select_only()
            .column(song_meta::Column::AlbumName)
            .filter(song_meta::Column::Namespace.eq(self.namespace()))
            .filter(song_meta::Column::AlbumId.eq(id.value()))
            .filter(song_meta::Column::AlbumName.is_not_null())
            .limit(/*limit*/ 1)
            .into_tuple::<String>()
            .one(db)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "回查专辑名",
                record: id.value().to_owned(),
                source,
            })
    }

    /// 按 artist id 回查艺名(取任一署名行 `song_artists.artist_name`;艺名同样只作为歌的
    /// 投影存)。降级 / 未命中返回 `Ok(None)`。
    ///
    /// # Params:
    ///   - `id`: 艺人 id(裸值查 `song_artists.artist_id`)
    ///
    /// # Return:
    ///   命中返回 `Ok(Some(name))`,否则 `Ok(None)`。
    pub async fn artist_name(&self, id: &ArtistId) -> crate::persistence::Result<Option<String>> {
        self.check_source(id.namespace())?;
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        song_artists::Entity::find()
            .select_only()
            .column(song_artists::Column::ArtistName)
            .filter(song_artists::Column::Namespace.eq(self.namespace()))
            .filter(song_artists::Column::ArtistId.eq(id.value()))
            .limit(/*limit*/ 1)
            .into_tuple::<String>()
            .one(db)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "回查艺名",
                record: id.value().to_owned(),
                source,
            })
    }

    /// 按状态 transition 设/取消一首歌的 favorite membership。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///   - `loved`: true=喜欢，false=取消
    ///
    /// # Return:
    ///   实际创建或删除 membership 返回 `true`；同状态 no-op 或降级返回 `false`。
    pub async fn set_loved(&self, id: &SongId, loved: bool) -> crate::persistence::Result<bool> {
        self.check_source(id.namespace())?;
        let Some(db) = self.pool() else {
            return Ok(false);
        };
        trace!(target: "persist", song = id.value(), loved, "set_loved");
        let changed = if loved {
            song_favorites::Entity::insert(song_favorites::ActiveModel {
                namespace: Set(self.namespace().to_owned()),
                song_value: Set(id.value().to_owned()),
                entered_at: Set(crate::persistence::db::time::now_ms()),
            })
            .on_conflict(
                OnConflict::columns([
                    song_favorites::Column::Namespace,
                    song_favorites::Column::SongValue,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec_without_returning(db)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "写收藏",
                record: id.value().to_owned(),
                source,
            })?
        } else {
            song_favorites::Entity::delete_by_id((
                self.namespace().to_owned(),
                id.value().to_owned(),
            ))
            .exec(db)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "取消收藏",
                record: id.value().to_owned(),
                source,
            })?
            .rows_affected
        };
        Ok(changed != 0)
    }

    /// 是否 loved。降级 / 无记录返回 false。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///
    /// # Return:
    ///   存在 favorite membership 时 true。
    pub async fn is_loved(&self, id: &SongId) -> crate::persistence::Result<bool> {
        self.check_source(id.namespace())?;
        let Some(db) = self.pool() else {
            return Ok(false);
        };
        Ok(
            song_favorites::Entity::find_by_id((
                self.namespace().to_owned(),
                id.value().to_owned(),
            ))
            .one(db)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "查收藏",
                record: id.value().to_owned(),
                source,
            })?
            .is_some(),
        )
    }

    /// 本来源全部 loved 歌 id 集合。降级返回空集。
    ///
    /// # Return:
    ///   本 namespace 的 favorite SongId 集合。
    pub async fn loved_ids(&self) -> crate::persistence::Result<rustc_hash::FxHashSet<SongId>> {
        let Some(db) = self.pool() else {
            return Ok(rustc_hash::FxHashSet::default());
        };
        let rows = song_favorites::Entity::find()
            .filter(song_favorites::Column::Namespace.eq(self.namespace()))
            .all(db)
            .await
            .map_err(|source| crate::persistence::Error::Database {
                operation: "列出收藏身份",
                source,
            })?;
        Ok(rows
            .into_iter()
            .map(|row| SongId::new(self.source, row.song_value))
            .collect())
    }

    /// 写歌单缓存(覆盖：upsert 元信息 + 先删后插 relation，刷新 fetched_at)。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌单 id
    ///   - `name`: 歌单名(可空)
    ///   - `track_update_time`: 歌单版本戳(网易云 `trackUpdateTime`,可空)
    ///   - `entries`: 显式 index + SongId relation，Song namespace 不由 Playlist 隐含
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn put_playlist_cache(
        &self,
        id: &PlaylistId,
        name: Option<&str>,
        track_update_time: Option<i64>,
        entries: &[CachedPlaylistEntry],
    ) -> crate::persistence::Result<()> {
        self.check_source(id.namespace())?;
        let Some(db) = self.pool() else {
            return Ok(());
        };
        let ns = self.namespace();
        let pid = id.value();
        trace!(target: "persist", playlist = pid, tracks = entries.len(), "put_playlist_cache");
        let tx = db
            .begin()
            .await
            .map_err(|source| crate::persistence::Error::Database {
                operation: "开启 put_playlist_cache 事务",
                source,
            })?;
        playlist_cache::Entity::insert(playlist_cache::ActiveModel {
            namespace: Set(ns.to_owned()),
            playlist_id: Set(pid.to_owned()),
            name: Set(name.map(str::to_owned)),
            fetched_at: Set(crate::persistence::db::time::now_ms()),
            track_update_time: Set(track_update_time),
        })
        .on_conflict(
            OnConflict::columns([
                playlist_cache::Column::Namespace,
                playlist_cache::Column::PlaylistId,
            ])
            .update_columns([
                playlist_cache::Column::Name,
                playlist_cache::Column::FetchedAt,
                playlist_cache::Column::TrackUpdateTime,
            ])
            .to_owned(),
        )
        .exec_without_returning(&tx)
        .await
        .map_err(|source| crate::persistence::Error::Record {
            operation: "upsert playlist_cache",
            record: pid.to_string(),
            source,
        })?;
        playlist_entries::Entity::delete_many()
            .filter(playlist_entries::Column::PlaylistNamespace.eq(ns))
            .filter(playlist_entries::Column::PlaylistValue.eq(pid))
            .exec(&tx)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "清空 playlist_entries",
                record: pid.to_string(),
                source,
            })?;
        for batch in entries.chunks(100) {
            let models = batch
                .iter()
                .map(|entry| {
                    Ok(playlist_entries::ActiveModel {
                        playlist_namespace: Set(ns.to_owned()),
                        playlist_value: Set(pid.to_owned()),
                        collection_index: Set(i64::try_from(entry.index.get())?),
                        song_namespace: Set(entry.song_id.namespace().name().to_owned()),
                        song_value: Set(entry.song_id.value().to_owned()),
                    })
                })
                .collect::<crate::persistence::Result<Vec<_>>>()?;
            playlist_entries::Entity::insert_many(models)
                .exec_without_returning(&tx)
                .await
                .map_err(|source| crate::persistence::Error::Record {
                    operation: "批量写入 playlist_entries",
                    record: pid.to_string(),
                    source,
                })?;
        }
        tx.commit()
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "提交 put_playlist_cache 事务",
                record: pid.to_string(),
                source,
            })?;
        Ok(())
    }

    /// 读歌单缓存(relation 按 collection index 升序)。降级 / 未命中返回 `None`。
    ///
    /// # Params:
    ///   - `id`: 歌单 id
    ///
    /// # Return:
    ///   命中返回缓存条目(含 fetched_at / track_update_time 供调用方做版本比对)，否则 None。
    pub async fn get_playlist_cache(
        &self,
        id: &PlaylistId,
    ) -> crate::persistence::Result<Option<PlaylistCacheEntry>> {
        self.check_source(id.namespace())?;
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        let ns = self.namespace();
        let pid = id.value();
        let Some(head) = playlist_cache::Entity::find_by_id((ns.to_owned(), pid.to_owned()))
            .one(db)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "查 playlist_cache",
                record: pid.to_string(),
                source,
            })?
        else {
            return Ok(None);
        };
        let rows = playlist_entries::Entity::find()
            .filter(playlist_entries::Column::PlaylistNamespace.eq(ns))
            .filter(playlist_entries::Column::PlaylistValue.eq(pid))
            .order_by_asc(playlist_entries::Column::CollectionIndex)
            .all(db)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "查 playlist_entries",
                record: pid.to_string(),
                source,
            })?;
        let entries = rows
            .into_iter()
            .map(|row| {
                Ok(CachedPlaylistEntry {
                    index: CollectionIndex::new(u64::try_from(row.collection_index)?),
                    song_id: SongId::new(
                        SourceKind::from_name(&row.song_namespace),
                        row.song_value,
                    ),
                })
            })
            .collect::<crate::persistence::Result<Vec<_>>>()?;
        Ok(Some(PlaylistCacheEntry {
            name: head.name,
            fetched_at: head.fetched_at,
            track_update_time: head.track_update_time,
            entries,
        }))
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{CollectionIndex, Song, SongId, SourceKind};

    use super::CachedPlaylistEntry;

    /// 构造 playlist cache relation fixture。
    fn cached(index: u64, source: SourceKind, value: &str) -> CachedPlaylistEntry {
        CachedPlaylistEntry {
            index: CollectionIndex::new(index),
            song_id: SongId::new(source, value),
        }
    }

    /// album_name / artist_name:按 id 回查名(取任一成员歌 / 署名行),未命中 None。
    #[tokio::test]
    async fn album_and_artist_name_lookup() -> color_eyre::Result<()> {
        use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef};

        let dir = tempfile::tempdir()?;
        let p = crate::persistence::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let song = Song::builder()
            .id(SongId::new(SourceKind::NETEASE, "1"))
            .name("稻香".to_owned())
            .artists(vec![ArtistRef {
                id: ArtistId::new(SourceKind::NETEASE, "jay"),
                name: "周杰伦".to_owned(),
            }])
            .album(Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "mojito"),
                name: "魔杰座".to_owned(),
            }))
            .build();
        s.upsert_meta(&song).await?;

        assert_eq!(
            s.album_name(&AlbumId::new(SourceKind::NETEASE, "mojito"))
                .await?,
            Some("魔杰座".to_owned())
        );
        assert_eq!(
            s.artist_name(&ArtistId::new(SourceKind::NETEASE, "jay"))
                .await?,
            Some("周杰伦".to_owned())
        );
        assert_eq!(
            s.album_name(&AlbumId::new(SourceKind::NETEASE, "absent"))
                .await?,
            None,
            "未命中回落 None"
        );
        Ok(())
    }

    #[tokio::test]
    async fn love_toggle_and_list() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::persistence::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "123");
        assert!(!s.is_loved(&id).await?);
        assert!(s.set_loved(&id, true).await?, "false -> true 应创建");
        assert!(!s.set_loved(&id, true).await?, "true -> true 应 no-op");
        assert!(s.is_loved(&id).await?);
        assert!(s.loved_ids().await?.contains(&id));
        assert!(s.set_loved(&id, false).await?, "true -> false 应删除");
        assert!(!s.set_loved(&id, false).await?, "false -> false 应 no-op");
        assert!(!s.is_loved(&id).await?);
        assert!(!s.loved_ids().await?.contains(&id));
        Ok(())
    }

    #[tokio::test]
    async fn loved_ids_isolated_by_source() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::persistence::ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = p.scope(SourceKind::NETEASE);
        let local = p.scope(SourceKind::LOCAL);
        netease
            .set_loved(&SongId::new(SourceKind::NETEASE, "n1"), true)
            .await?;
        assert_eq!(netease.loved_ids().await?.len(), 1);
        assert_eq!(local.loved_ids().await?.len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn playlist_cache_roundtrip() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::persistence::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let pid = mineral_model::PlaylistId::new(SourceKind::NETEASE, "p1");
        let entries = vec![
            cached(0, SourceKind::NETEASE, "s1"),
            cached(3, SourceKind::BILIBILI, "s2"),
            cached(9, SourceKind::NETEASE, "s3"),
        ];
        s.put_playlist_cache(&pid, Some("我的歌单"), Some(1_775_781_450_653), &entries)
            .await?;
        let got = s.get_playlist_cache(&pid).await?;
        assert!(got.is_some());
        if let Some(g) = got {
            assert_eq!(g.name, Some("我的歌单".to_owned()));
            assert_eq!(
                g.entries, entries,
                "index gap 与 mixed-source SongId 原值保留"
            );
            assert!(g.fetched_at > 0);
            assert_eq!(g.track_update_time, Some(1_775_781_450_653)); // 版本戳 roundtrip
        }
        Ok(())
    }

    #[tokio::test]
    async fn playlist_cache_overwrite_clears_old_tracks() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::persistence::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let pid = mineral_model::PlaylistId::new(SourceKind::NETEASE, "p1");
        s.put_playlist_cache(
            &pid,
            Some("v1"),
            Some(100),
            &[
                cached(0, SourceKind::NETEASE, "a"),
                cached(1, SourceKind::NETEASE, "b"),
            ],
        )
        .await?;
        s.put_playlist_cache(
            &pid,
            Some("v2"),
            Some(200),
            &[cached(5, SourceKind::BILIBILI, "c")],
        )
        .await?; // 覆盖
        let got = s.get_playlist_cache(&pid).await?;
        assert!(got.is_some());
        if let Some(g) = got {
            assert_eq!(g.name, Some("v2".to_owned()));
            assert_eq!(
                g.entries,
                vec![cached(5, SourceKind::BILIBILI, "c")],
                "旧 a,b 不残留，explicit index/source 保留"
            );
            assert_eq!(g.track_update_time, Some(200)); // 版本戳也被覆盖刷新
        }
        Ok(())
    }

    #[tokio::test]
    async fn playlist_cache_miss_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::persistence::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let pid = mineral_model::PlaylistId::new(SourceKind::NETEASE, "absent");
        assert!(s.get_playlist_cache(&pid).await?.is_none());
        Ok(())
    }
}
