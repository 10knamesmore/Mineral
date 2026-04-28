use serde::{Deserialize, Serialize};

use crate::{ids::PlaylistId, song::Song, source::SourceKind, url::MediaUrl};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub source: SourceKind,
    pub id: PlaylistId,
    pub name: String,
    pub description: String,
    pub cover_url: Option<MediaUrl>,
    pub track_count: u64,
    pub songs: Vec<Song>,
}
