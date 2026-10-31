use std::str::FromStr;

use mineral_model::{
    Album, AlbumId, ArtistId, ArtistRef, MediaUrl, Playlist, PlaylistActions, PlaylistId,
};
use rustc_hash::FxHashMap;
use sea_orm::{ColumnTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, TransactionTrait};
use url::Url;

use crate::persistence::db::NamespaceStore;
use crate::persistence::entity::library_lists::{
    album_artists, albums, playlists, requested_albums, requested_playlists,
};
use crate::persistence::{Error, Result};

impl NamespaceStore {
    /// 读取同一快照内的歌单列表；未缓存返回 None，entries 留空。
    pub async fn get_my_playlists_cache(&self) -> Result<Option<Vec<Playlist>>> {
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        let tx = db.begin().await.map_err(playlist_database_error)?;
        let requested = requested_playlists::Entity::find_by_id(self.namespace().to_owned())
            .one(&tx)
            .await
            .map_err(playlist_database_error)?;
        if requested.is_none() {
            tx.commit().await.map_err(playlist_database_error)?;
            return Ok(None);
        }
        let rows = playlists::Entity::find()
            .filter(playlists::Column::Namespace.eq(self.namespace()))
            .order_by_asc(playlists::Column::Position)
            .all(&tx)
            .await
            .map_err(playlist_database_error)?;
        tx.commit().await.map_err(playlist_database_error)?;

        let items = rows
            .into_iter()
            .map(|row| {
                Ok(Playlist::builder()
                    .id(PlaylistId::new(self.source(), row.playlist_id))
                    .name(row.name)
                    .actions(PlaylistActions {
                        rename: row.can_rename,
                        delete: row.can_delete,
                    })
                    .description(row.description)
                    .cover_url(row.cover_url.as_deref().map(decode_media_url))
                    .web_url(row.web_url.as_deref().map(Url::parse).transpose()?)
                    .track_count(u64::try_from(row.track_count)?)
                    .play_count(row.play_count.map(u64::try_from).transpose()?)
                    .subscriber_count(row.subscriber_count.map(u64::try_from).transpose()?)
                    .entries(Vec::new())
                    .build())
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(items))
    }

    /// 读取同一快照内的专辑及艺人列表；未缓存返回 None，tracks 留空。
    pub async fn get_my_albums_cache(&self) -> Result<Option<Vec<Album>>> {
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        let tx = db.begin().await.map_err(album_database_error)?;
        let requested = requested_albums::Entity::find_by_id(self.namespace().to_owned())
            .one(&tx)
            .await
            .map_err(album_database_error)?;
        if requested.is_none() {
            tx.commit().await.map_err(album_database_error)?;
            return Ok(None);
        }
        let rows = albums::Entity::find()
            .filter(albums::Column::Namespace.eq(self.namespace()))
            .order_by_asc(albums::Column::Position)
            .all(&tx)
            .await
            .map_err(album_database_error)?;
        let artists = album_artists::Entity::find()
            .filter(album_artists::Column::Namespace.eq(self.namespace()))
            .order_by_asc(album_artists::Column::AlbumPosition)
            .order_by_asc(album_artists::Column::Position)
            .all(&tx)
            .await
            .map_err(album_database_error)?;
        tx.commit().await.map_err(album_database_error)?;

        let mut artists_by_album = FxHashMap::<i64, Vec<ArtistRef>>::default();
        for artist in artists {
            artists_by_album
                .entry(artist.album_position)
                .or_default()
                .push(ArtistRef {
                    id: ArtistId::new(self.source(), artist.artist_id),
                    name: artist.name,
                });
        }
        let items = rows
            .into_iter()
            .map(|row| {
                Ok(Album::builder()
                    .id(AlbumId::new(self.source(), row.album_id))
                    .name(row.name)
                    .description(row.description)
                    .company(row.company)
                    .publish_time_ms(row.publish_time_ms)
                    .track_count(row.track_count.map(u64::try_from).transpose()?)
                    .cover_url(row.cover_url.as_deref().map(decode_media_url))
                    .web_url(row.web_url.as_deref().map(Url::parse).transpose()?)
                    .artists(artists_by_album.remove(&row.position).unwrap_or_default())
                    .tracks(Vec::new())
                    .build())
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(items))
    }
}

fn playlist_database_error(source: DbErr) -> Error {
    Error::Database {
        operation: "read my playlists cache",
        source,
    }
}

fn album_database_error(source: DbErr) -> Error {
    Error::Database {
        operation: "read my albums cache",
        source,
    }
}

fn decode_media_url(value: &str) -> MediaUrl {
    match MediaUrl::from_str(value) {
        Ok(url) => url,
        Err(never) => match never {},
    }
}
