//! 持久化记录与歌曲模型之间的转换。

use std::str::FromStr;

use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef, MediaUrl, Song, SongId, SourceKind};
use sea_orm::FromQueryResult;

pub(crate) use crate::entity::song_meta::Model as SongMetaRow;

/// `song_artists` 行(仅取重建艺人所需字段)。
#[derive(FromQueryResult)]
pub(crate) struct SongArtistRow {
    /// 艺人裸 id。
    pub artist_id: String,

    /// 艺名。
    pub artist_name: String,
}

impl SongMetaRow {
    /// 与按 `position` 升序排好的艺人行一起重建 [`Song`](namespace 从行内还原)。
    ///
    /// # Params:
    ///   - `artists`: 该歌的艺人行(调用方保证顺序)
    ///
    /// # Return:
    ///   重建的 [`Song`];`duration_ms` 负值(库损坏)时报错。
    pub(crate) fn into_song(self, artists: Vec<SongArtistRow>) -> color_eyre::Result<Song> {
        let source = SourceKind::from_name(&self.namespace);
        let artists = artists
            .into_iter()
            .map(|a| ArtistRef {
                id: ArtistId::new(source, a.artist_id),
                name: a.artist_name,
            })
            .collect::<Vec<ArtistRef>>();

        let album = match (self.album_id, self.album_name) {
            (Some(aid), Some(aname)) => Some(AlbumRef {
                id: AlbumId::new(source, aid),
                name: aname,
            }),
            _ => None,
        };

        let cover_url = self.cover_url.map(|s| match MediaUrl::from_str(&s) {
            Ok(u) => u,
            Err(never) => match never {},
        });

        let duration_ms = self.duration_ms.map(u64::try_from).transpose()?;

        Ok(Song::builder()
            .id(SongId::new(source, self.song_value))
            .name(self.name)
            .alias(self.alias)
            .artists(artists)
            .album(album)
            .duration_ms(duration_ms)
            .cover_url(cover_url)
            .build())
    }
}
