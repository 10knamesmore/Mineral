//! Identity resolution does not acquire media; each prepared track opens once.

use std::sync::Arc;

use async_trait::async_trait;
use mineral_model::{BitRate, DirectMedia, SongId};
use mineral_playback::{OpenOptions, OpenedMedia, PlaybackRequest, PreparedPlayback};
use tokio_util::sync::CancellationToken;

use super::open;
use super::settings::PlaybackSettings;
use crate::account::AccountClient;
use crate::metadata::{MetadataClient, SongIdentity};
use crate::{AppleMusicConfig, Error};

/// A resolved identity and the account/settings needed when playback is opened.
/// No playlist, license or media requests have started yet.
struct PreparedTrack {
    account: Arc<AccountClient>,

    song_id: SongId,

    identity: SongIdentity,

    quality: BitRate,

    settings: PlaybackSettings,
}

pub(crate) async fn prepare(
    metadata: &MetadataClient,
    account: Arc<AccountClient>,
    config: &AppleMusicConfig,
    request: PlaybackRequest,
    cancellation: CancellationToken,
) -> mineral_playback::Result<Box<dyn PreparedPlayback>> {
    let resolve = async {
        let identity = metadata.song_identity(request.song_id()).await?;
        let plan: Box<dyn PreparedPlayback> = Box::new(PreparedTrack {
            account,
            song_id: request.song_id().clone(),
            identity,
            quality: request.quality(),
            settings: PlaybackSettings::from(config),
        });
        Ok::<_, Error>(plan)
    };
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(mineral_playback::Error::Cancelled),
        result = resolve => result.map_err(Into::into),
    }
}

#[async_trait]
impl PreparedPlayback for PreparedTrack {
    fn direct_media(&self) -> Option<&DirectMedia> {
        None
    }

    async fn open(self: Box<Self>, options: OpenOptions) -> mineral_playback::Result<OpenedMedia> {
        let cancellation = options.cancellation().clone();
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(mineral_playback::Error::Cancelled),
            result = open::open(self.account, self.song_id, self.identity, self.quality, self.settings, options) => result.map_err(Into::into),
        }
    }
}
