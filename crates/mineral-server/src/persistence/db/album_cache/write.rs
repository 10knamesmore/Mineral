//! 一次事务替换专辑元信息、曲目及各自的艺人快照。

use mineral_channel_core::store::AlbumCacheEntry;
use mineral_model::MediaUrl;
use sea_orm::{DbErr, EntityTrait, Set, TransactionTrait};

use crate::persistence::db::NamespaceStore;
use crate::persistence::entity::album_cache::{albums, artists, track_artists, tracks};
use crate::persistence::{Error, Result};

/// 最宽的曲目行也不会超过 SQLite 单次语句的参数数量限制。
const INSERT_BATCH_SIZE: usize = 50;

impl NamespaceStore {
    /// 完整替换一个专辑缓存；禁用存储丢弃合法写入，所有身份先校验来源。
    pub async fn put_album_cache(&self, entry: &AlbumCacheEntry) -> Result<()> {
        let album = &entry.album;
        self.check_source(album.source())?;
        for artist in &album.artists {
            self.check_source(artist.id.namespace())?;
        }
        for track in &album.tracks {
            self.check_song(&track.song)?;
        }
        let Some(db) = self.pool() else {
            return Ok(());
        };
        let namespace = self.namespace();
        let album_id = album.id.value();
        let header = albums::ActiveModel {
            namespace: Set(namespace.to_owned()),
            album_id: Set(album_id.to_owned()),
            name: Set(album.name.clone()),
            description: Set(album.description.clone()),
            company: Set(album.company.clone()),
            publish_time_ms: Set(album.publish_time_ms),
            track_count: Set(album.track_count.map(i64::try_from).transpose()?),
            cover_url: Set(album.cover_url.as_ref().map(MediaUrl::to_string)),
            expired_at: Set(entry.expired_at),
        };
        let album_artists = album
            .artists
            .iter()
            .enumerate()
            .map(|(position, artist)| {
                Ok(artists::ActiveModel {
                    namespace: Set(namespace.to_owned()),
                    album_id: Set(album_id.to_owned()),
                    position: Set(i64::try_from(position)?),
                    artist_id: Set(artist.id.value().to_owned()),
                    name: Set(artist.name.clone()),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut album_tracks = Vec::with_capacity(album.tracks.len());
        let mut song_artists = Vec::new();
        for track in &album.tracks {
            let collection_index = i64::try_from(track.index.get())?;
            let song = &track.song;
            album_tracks.push(tracks::ActiveModel {
                namespace: Set(namespace.to_owned()),
                album_id: Set(album_id.to_owned()),
                collection_index: Set(collection_index),
                song_value: Set(song.id.value().to_owned()),
                name: Set(song.name.clone()),
                alias: Set(song.alias.clone()),
                song_album_id: Set(song.album.as_ref().map(|a| a.id.value().to_owned())),
                song_album_name: Set(song.album.as_ref().map(|a| a.name.clone())),
                duration_ms: Set(song.duration_ms.map(i64::try_from).transpose()?),
                cover_url: Set(song.cover_url.as_ref().map(MediaUrl::to_string)),
                source_url: Set(song.source_url.as_ref().map(MediaUrl::to_string)),
                unavailable: Set(song.unavailable),
            });
            for (position, artist) in song.artists.iter().enumerate() {
                song_artists.push(track_artists::ActiveModel {
                    namespace: Set(namespace.to_owned()),
                    album_id: Set(album_id.to_owned()),
                    collection_index: Set(collection_index),
                    position: Set(i64::try_from(position)?),
                    artist_id: Set(artist.id.value().to_owned()),
                    name: Set(artist.name.clone()),
                });
            }
        }
        let tx = db.begin().await.map_err(database_error)?;
        // 外键级联清理旧关系，与新快照一起提交，读者不会看到新旧混合的数据。
        albums::Entity::delete_by_id((namespace.to_owned(), album_id.to_owned()))
            .exec(&tx)
            .await
            .map_err(database_error)?;
        albums::Entity::insert(header)
            .exec_without_returning(&tx)
            .await
            .map_err(database_error)?;
        for batch in album_artists.chunks(INSERT_BATCH_SIZE) {
            artists::Entity::insert_many(batch.to_vec())
                .exec_without_returning(&tx)
                .await
                .map_err(database_error)?;
        }
        for batch in album_tracks.chunks(INSERT_BATCH_SIZE) {
            tracks::Entity::insert_many(batch.to_vec())
                .exec_without_returning(&tx)
                .await
                .map_err(database_error)?;
        }
        for batch in song_artists.chunks(INSERT_BATCH_SIZE) {
            track_artists::Entity::insert_many(batch.to_vec())
                .exec_without_returning(&tx)
                .await
                .map_err(database_error)?;
        }
        tx.commit().await.map_err(database_error)?;
        mineral_log::debug!(target: "persist", album = %album.id, tracks = album.tracks.len(),
            expired_at = %entry.expired_at, "saved album cache snapshot");
        Ok(())
    }
}

/// 保留专辑缓存替换操作和数据库原因。
fn database_error(source: DbErr) -> Error {
    Error::Database {
        operation: "replace album cache",
        source,
    }
}
