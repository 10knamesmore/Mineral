use serde::{Deserialize, Serialize};

use crate::{ids::AlbumId, refs::ArtistRef, song::Song, source::SourceKind, url::MediaUrl};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    pub source: SourceKind,
    pub id: AlbumId,
    pub name: String,
    pub artists: Vec<ArtistRef>,
    pub description: String,
    pub publish_time_ms: i64,
    pub cover_url: Option<MediaUrl>,
    pub songs: Vec<Song>,
}
