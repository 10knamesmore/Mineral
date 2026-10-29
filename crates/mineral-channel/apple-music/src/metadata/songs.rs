//! Song details and catalog/library identity resolution without acquiring media.

use mineral_model::{Song, SongId};

use super::{MetadataClient, mapping};
use crate::{Error, Result};

/// The Apple identity accepted by playback asset requests.
pub(crate) enum SongIdentity {
    /// A catalog recording, including a library song's catalog equivalent.
    Catalog(String),

    /// A library recording without a mapped catalog resource.
    Library(String),
}

impl MetadataClient {
    /// Preserve the requested IDs even when metadata comes from a catalog equivalent.
    pub(crate) async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>> {
        let mut songs = Vec::with_capacity(ids.len());
        for id in ids {
            let resource = self.resource("songs", id.as_str()).await?;
            let mut song = mapping::song(&resource)?;
            song.id = id.clone();
            songs.push(song);
        }
        Ok(songs)
    }

    /// Numeric catalog IDs need no lookup; library IDs may resolve to a catalog recording.
    pub(crate) async fn song_identity(&self, id: &SongId) -> Result<SongIdentity> {
        let value = id.as_str();
        if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok(SongIdentity::Catalog(value.to_owned()));
        }
        let resource = self.resource("songs", value).await?;
        let song = mapping::song_resource(&resource);
        match song.kind.as_str() {
            "songs" => Ok(SongIdentity::Catalog(song.id.clone())),
            "library-songs" => Ok(SongIdentity::Library(song.id.clone())),
            _ => Err(Error::InvalidData {
                field: "playback song identity",
            }),
        }
    }
}
