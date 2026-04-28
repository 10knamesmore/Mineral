use serde::{Deserialize, Serialize};

use crate::{ids::ArtistId, song::Song, source::SourceKind, url::MediaUrl};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub source: SourceKind,
    pub id: ArtistId,
    pub name: String,
    pub description: String,
    pub follower_count: u64,
    pub avatar_url: Option<MediaUrl>,
    pub songs: Vec<Song>,
}
