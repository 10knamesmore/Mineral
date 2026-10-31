use mineral_model::{Album, MediaUrl, Playlist};
use sea_orm::{DbErr, EntityTrait, Set, TransactionTrait};
use url::Url;

use crate::persistence::db::NamespaceStore;
use crate::persistence::entity::library_lists::{
    album_artists, albums, playlists, requested_albums, requested_playlists,
};
use crate::persistence::{Error, Result};

/// 每批参数数低于 SQLite 的语句参数上限。
const INSERT_BATCH_SIZE: usize = 50;

impl NamespaceStore {
    /// 事务替换原始歌单列表，保留顺序及元信息，忽略 entries。
    pub async fn put_my_playlists_cache(&self, items: &[Playlist]) -> Result<()> {
        for playlist in items {
            self.check_source(playlist.source())?;
        }
        let Some(db) = self.pool() else {
            return Ok(());
        };
        let namespace = self.namespace();
        let rows = items
            .iter()
            .enumerate()
            .map(|(position, playlist)| {
                Ok(playlists::ActiveModel {
                    namespace: Set(namespace.to_owned()),
                    position: Set(i64::try_from(position)?),
                    playlist_id: Set(playlist.id.value().to_owned()),
                    name: Set(playlist.name.clone()),
                    can_rename: Set(playlist.actions.rename),
                    can_delete: Set(playlist.actions.delete),
                    description: Set(playlist.description.clone()),
                    cover_url: Set(playlist.cover_url.as_ref().map(encode_media_url)),
                    web_url: Set(playlist.web_url.as_ref().map(Url::to_string)),
                    track_count: Set(i64::try_from(playlist.track_count)?),
                    play_count: Set(playlist.play_count.map(i64::try_from).transpose()?),
                    subscriber_count: Set(playlist
                        .subscriber_count
                        .map(i64::try_from)
                        .transpose()?),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let tx = db.begin().await.map_err(playlist_database_error)?;
        // 删除快照会级联删除旧列表；即使新列表为空，也插入快照记录。
        requested_playlists::Entity::delete_by_id(namespace.to_owned())
            .exec(&tx)
            .await
            .map_err(playlist_database_error)?;
        requested_playlists::Entity::insert(requested_playlists::ActiveModel {
            namespace: Set(namespace.to_owned()),
        })
        .exec_without_returning(&tx)
        .await
        .map_err(playlist_database_error)?;
        for batch in rows.chunks(INSERT_BATCH_SIZE) {
            playlists::Entity::insert_many(batch.to_vec())
                .exec_without_returning(&tx)
                .await
                .map_err(playlist_database_error)?;
        }
        tx.commit().await.map_err(playlist_database_error)?;
        Ok(())
    }

    /// 事务替换原始专辑列表，保留专辑及艺人顺序，忽略 tracks。
    pub async fn put_my_albums_cache(&self, items: &[Album]) -> Result<()> {
        for album in items {
            self.check_source(album.source())?;
            for artist in &album.artists {
                self.check_source(artist.id.namespace())?;
            }
        }
        let Some(db) = self.pool() else {
            return Ok(());
        };
        let namespace = self.namespace();
        let mut rows = Vec::with_capacity(items.len());
        let mut artists = Vec::new();
        for (position, album) in items.iter().enumerate() {
            let album_position = i64::try_from(position)?;
            rows.push(albums::ActiveModel {
                namespace: Set(namespace.to_owned()),
                position: Set(album_position),
                album_id: Set(album.id.value().to_owned()),
                name: Set(album.name.clone()),
                description: Set(album.description.clone()),
                company: Set(album.company.clone()),
                publish_time_ms: Set(album.publish_time_ms),
                track_count: Set(album.track_count.map(i64::try_from).transpose()?),
                cover_url: Set(album.cover_url.as_ref().map(encode_media_url)),
                web_url: Set(album.web_url.as_ref().map(Url::to_string)),
            });
            for (position, artist) in album.artists.iter().enumerate() {
                artists.push(album_artists::ActiveModel {
                    namespace: Set(namespace.to_owned()),
                    album_position: Set(album_position),
                    position: Set(i64::try_from(position)?),
                    artist_id: Set(artist.id.value().to_owned()),
                    name: Set(artist.name.clone()),
                });
            }
        }

        let tx = db.begin().await.map_err(album_database_error)?;
        requested_albums::Entity::delete_by_id(namespace.to_owned())
            .exec(&tx)
            .await
            .map_err(album_database_error)?;
        requested_albums::Entity::insert(requested_albums::ActiveModel {
            namespace: Set(namespace.to_owned()),
        })
        .exec_without_returning(&tx)
        .await
        .map_err(album_database_error)?;
        for batch in rows.chunks(INSERT_BATCH_SIZE) {
            albums::Entity::insert_many(batch.to_vec())
                .exec_without_returning(&tx)
                .await
                .map_err(album_database_error)?;
        }
        for batch in artists.chunks(INSERT_BATCH_SIZE) {
            album_artists::Entity::insert_many(batch.to_vec())
                .exec_without_returning(&tx)
                .await
                .map_err(album_database_error)?;
        }
        tx.commit().await.map_err(album_database_error)?;
        Ok(())
    }
}

fn playlist_database_error(source: DbErr) -> Error {
    Error::Database {
        operation: "replace my playlists cache",
        source,
    }
}

fn album_database_error(source: DbErr) -> Error {
    Error::Database {
        operation: "replace my albums cache",
        source,
    }
}

/// 本地路径显式携带 file:// 前缀，读取时保留 MediaUrl 的本地类型。
fn encode_media_url(url: &MediaUrl) -> String {
    match url {
        MediaUrl::Remote(url) => url.to_string(),
        MediaUrl::Local(path) => format!("file://{}", path.display()),
    }
}
