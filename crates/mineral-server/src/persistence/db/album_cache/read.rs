//! 同一读取事务内重建专辑详情，保留曲目及艺人的原始顺序。

use std::str::FromStr;

use mineral_channel_core::store::AlbumCacheEntry;
use mineral_model::{
    Album, AlbumId, AlbumRef, AlbumTrack, ArtistId, ArtistRef, CollectionIndex, MediaUrl, Song,
    SongId,
};
use rustc_hash::FxHashMap;
use sea_orm::{ColumnTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, TransactionTrait};

use crate::persistence::db::NamespaceStore;
use crate::persistence::entity::album_cache::{albums, artists, track_artists, tracks};
use crate::persistence::{Error, Result};

impl NamespaceStore {
    /// 读取完整快照和过期时间；空专辑也是有效缓存，过期由 channel 判断。
    pub async fn get_album_cache(&self, id: &AlbumId) -> Result<Option<AlbumCacheEntry>> {
        self.check_source(id.namespace())?;
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        let tx = db.begin().await.map_err(database_error)?;
        let Some(header) =
            albums::Entity::find_by_id((self.namespace().to_owned(), id.value().to_owned()))
                .one(&tx)
                .await
                .map_err(database_error)?
        else {
            return Ok(None);
        };
        let album_artists = artists::Entity::find()
            .filter(artists::Column::Namespace.eq(self.namespace()))
            .filter(artists::Column::AlbumId.eq(id.value()))
            .order_by_asc(artists::Column::Position)
            .all(&tx)
            .await
            .map_err(database_error)?;
        let album_tracks = tracks::Entity::find()
            .filter(tracks::Column::Namespace.eq(self.namespace()))
            .filter(tracks::Column::AlbumId.eq(id.value()))
            .order_by_asc(tracks::Column::CollectionIndex)
            .all(&tx)
            .await
            .map_err(database_error)?;
        let song_artists = track_artists::Entity::find()
            .filter(track_artists::Column::Namespace.eq(self.namespace()))
            .filter(track_artists::Column::AlbumId.eq(id.value()))
            .order_by_asc(track_artists::Column::CollectionIndex)
            .order_by_asc(track_artists::Column::Position)
            .all(&tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;

        let source = self.source();
        let mut artists_by_track = FxHashMap::<i64, Vec<ArtistRef>>::default();
        for artist in song_artists {
            artists_by_track
                .entry(artist.collection_index)
                .or_default()
                .push(ArtistRef {
                    id: ArtistId::new(source, artist.artist_id),
                    name: artist.name,
                });
        }
        let album_tracks = album_tracks
            .into_iter()
            .map(|track| {
                let song_album =
                    track
                        .song_album_id
                        .zip(track.song_album_name)
                        .map(|(id, name)| AlbumRef {
                            id: AlbumId::new(source, id),
                            name,
                        });
                let song = Song::builder()
                    .id(SongId::new(source, track.song_value))
                    .name(track.name)
                    .alias(track.alias)
                    .artists(
                        artists_by_track
                            .remove(&track.collection_index)
                            .unwrap_or_default(),
                    )
                    .album(song_album)
                    .duration_ms(track.duration_ms.map(u64::try_from).transpose()?)
                    .cover_url(track.cover_url.as_deref().map(parse_url))
                    .source_url(track.source_url.as_deref().map(parse_url))
                    .unavailable(track.unavailable)
                    .build();
                Ok(AlbumTrack::builder()
                    .index(CollectionIndex::new(u64::try_from(track.collection_index)?))
                    .song(song)
                    .build())
            })
            .collect::<Result<Vec<_>>>()?;
        let album = Album::builder()
            .id(id.clone())
            .name(header.name)
            .description(header.description)
            .company(header.company)
            .publish_time_ms(header.publish_time_ms)
            .track_count(header.track_count.map(u64::try_from).transpose()?)
            .cover_url(header.cover_url.as_deref().map(parse_url))
            .artists(
                album_artists
                    .into_iter()
                    .map(|artist| ArtistRef {
                        id: ArtistId::new(source, artist.artist_id),
                        name: artist.name,
                    })
                    .collect(),
            )
            .tracks(album_tracks)
            .build();
        Ok(Some(AlbumCacheEntry {
            album,
            expired_at: header.expired_at,
        }))
    }
}

/// 还原存储的媒体位置；MediaUrl 的字符串转换不会失败。
fn parse_url(value: &str) -> MediaUrl {
    match MediaUrl::from_str(value) {
        Ok(url) => url,
        Err(never) => match never {},
    }
}

/// 保留专辑缓存读取操作和数据库原因。
fn database_error(source: DbErr) -> Error {
    Error::Database {
        operation: "read album cache",
        source,
    }
}
