//! Opens the selected rendition and attaches indexed media and independent cache production.

use std::sync::Arc;
use std::time::Instant;

use mineral_model::{AudioFormat, DirectMedia, PlaybackMediaInfo, SongId, StreamLayout};
use mineral_playback::{DirectPreparedPlayback, OpenOptions, OpenedMedia, ReadControl};
use zeroize::Zeroizing;

use super::cenc::AudioInitialization;
use super::error::{Error, Result};
use super::store::MediaStore;
use super::{index, transport};
use crate::api::Api;

pub(crate) enum PlaybackIdentity {
    Catalog(String),

    Library(String),
}

pub(crate) async fn open(
    api: Arc<Api>,
    song_id: SongId,
    identity: PlaybackIdentity,
    options: OpenOptions,
) -> Result<OpenedMedia> {
    let started = Instant::now();
    api.session().await?;
    let asset = transport::asset(&api, &identity).await?;
    mineral_log::debug!(target: "apple_music", stage = "asset", elapsed_ms = started.elapsed().as_millis(), "playback preparation");
    let mut info = PlaybackMediaInfo {
        song_id,
        bitrate_bps: None,
        size: None,
        format: Some(AudioFormat::Mp4),
        bit_depth: None,
        substituted: false,
    };
    let PlaybackIdentity::Catalog(track_id) = identity else {
        let prepared = DirectPreparedPlayback::boxed(DirectMedia::remote(
            info,
            asset,
            Vec::new(),
            StreamLayout::Contiguous,
        ));
        return prepared
            .open(options)
            .await
            .map_err(|detail| Error::Upload { detail });
    };
    let manifest = transport::manifest(&api.client, &asset).await?;
    mineral_log::debug!(target: "apple_music", stage = "manifest", segments = manifest.segments.len(), elapsed_ms = started.elapsed().as_millis(), "playback preparation");
    let initialization = async {
        let bytes = transport::download(&api.client, &manifest.initialization).await?;
        Ok::<_, Error>(AudioInitialization::parse(&bytes)?)
    };
    let (initialization, keys, fragments) = tokio::try_join!(
        initialization,
        transport::content_keys(&api, &track_id, &manifest.key_uri),
        index::read_fragment_headers(
            &api.client,
            &manifest.segments,
            *api.config.media_concurrency(),
            *api.config.fragment_header_prefetch_bytes(),
        ),
    )?;
    mineral_log::debug!(target: "apple_music", stage = "metadata_and_license", elapsed_ms = started.elapsed().as_millis(), "playback preparation");
    let key = keys
        .iter()
        .find(|key| key.id.as_slice() == initialization.key_id())
        .ok_or(Error::InvalidMedia {
            field: "matching content key",
        })?;
    let key: [u8; 16] =
        key.bytes
            .as_slice()
            .try_into()
            .map_err(|_wrong_length| Error::InvalidMedia {
                field: "AES-128 content key",
            })?;
    let index = index::build(manifest.segments, fragments, &initialization)?;
    mineral_log::info!(target: "apple_music", song_id = %info.song_id.qualified(), elapsed_ms = started.elapsed().as_millis(), "indexed media ready");
    info.size = Some(index.byte_len);
    info.bitrate_bps = index.bitrate_bps;
    let store = MediaStore::new(
        index,
        initialization,
        Zeroizing::new(key),
        api.client.clone(),
        options.cancellation().clone(),
    )
    .await?;
    let control = ReadControl::new(options.cancellation().clone());
    let reader = store.reader(Some(control.clone()));
    let media = OpenedMedia::new(
        Box::new(reader),
        Some(store.byte_len),
        info,
        Some(store.transfer.clone()),
        options.cancellation().clone(),
    )
    .with_read_control(control);
    Ok(match options.capture_target() {
        Some(target) => media.with_capture(store.capture(target)),
        None => media,
    })
}
