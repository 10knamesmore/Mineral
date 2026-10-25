//! Resolves Apple identities without acquiring media; opening owns all media preparation.

use std::sync::Arc;

use async_trait::async_trait;
use mineral_model::{DirectMedia, SongId, SourceKind};
use mineral_playback::{
    OpenOptions, OpenedMedia, PlaybackProvider, PlaybackRequest, PreparedPlayback,
};
use tokio_util::sync::CancellationToken;

use crate::api::Api;
use crate::mapping::song_resource;
use crate::media::{self, PlaybackIdentity};
use crate::{AppleMusicChannel, Error, Result};

struct AppleMusicPlayback {
    api: Arc<Api>,

    song_id: SongId,

    identity: PlaybackIdentity,
}

#[async_trait]
impl PlaybackProvider for AppleMusicChannel {
    fn source(&self) -> SourceKind {
        SourceKind::APPLE_MUSIC
    }

    async fn resolve(
        &self,
        request: PlaybackRequest,
        cancellation: CancellationToken,
    ) -> mineral_playback::Result<Box<dyn PreparedPlayback>> {
        let resolve = async {
            let identity = playback_identity(&self.api, request.song_id()).await?;
            let plan: Box<dyn PreparedPlayback> = Box::new(AppleMusicPlayback {
                api: self.api.clone(),
                song_id: request.song_id().clone(),
                identity,
            });
            Ok::<_, Error>(plan)
        };
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(mineral_playback::Error::Cancelled),
            result = resolve => result.map_err(Into::into),
        }
    }
}

/// Catalog IDs already identify playback assets; library IDs may have a catalog equivalent.
async fn playback_identity(api: &Api, id: &SongId) -> Result<PlaybackIdentity> {
    let value = id.as_str();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(PlaybackIdentity::Catalog(value.to_owned()));
    }
    let resource = api.resource("songs", value).await?;
    let song = song_resource(&resource);
    match song.kind.as_str() {
        "songs" => Ok(PlaybackIdentity::Catalog(song.id.clone())),
        "library-songs" => Ok(PlaybackIdentity::Library(song.id.clone())),
        _ => Err(Error::InvalidData {
            field: "playback song identity",
        }),
    }
}

#[async_trait]
impl PreparedPlayback for AppleMusicPlayback {
    fn direct_media(&self) -> Option<&DirectMedia> {
        None
    }

    async fn open(self: Box<Self>, options: OpenOptions) -> mineral_playback::Result<OpenedMedia> {
        let cancellation = options.cancellation().clone();
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(mineral_playback::Error::Cancelled),
            result = media::open(self.api, self.song_id, self.identity, options) => result.map_err(Into::into),
        }
    }
}
