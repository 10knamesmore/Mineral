//! Opens the selected rendition with progressive audio and complete-file export.

use std::sync::Arc;
use std::time::Instant;

use mineral_model::{AudioFormat, BitRate, DirectMedia, PlaybackMediaInfo, SongId, StreamLayout};
use mineral_playback::{DirectPreparedPlayback, OpenOptions, OpenedMedia, ReadControl};
use zeroize::Zeroizing;

use super::cenc::{AudioInitialization, Error as EncryptionError};
use super::error::{Error, Result};
use super::packets::AudioPackets;
use super::store::MediaStore;
use super::transport;
use crate::api::Api;

pub(crate) enum PlaybackIdentity {
    Catalog(String),

    Library(String),
}

pub(crate) async fn open(
    api: Arc<Api>,
    song_id: SongId,
    identity: PlaybackIdentity,
    quality: BitRate,
    options: OpenOptions,
) -> Result<OpenedMedia> {
    let started = Instant::now();
    api.session().await?;
    let asset = transport::asset(&api, &identity, quality).await?;
    mineral_log::debug!(target: "apple_music", stage = "asset", elapsed_ms = started.elapsed().as_millis(), "playback preparation");
    let info = PlaybackMediaInfo {
        song_id,
        bitrate_bps: asset.bitrate_bps,
        size: None,
        format: Some(AudioFormat::Mp4),
        bit_depth: None,
        substituted: false,
    };
    let PlaybackIdentity::Catalog(track_id) = identity else {
        let prepared = DirectPreparedPlayback::boxed(DirectMedia::remote(
            info,
            asset.url,
            Vec::new(),
            StreamLayout::Contiguous,
        ));
        return prepared
            .open(options)
            .await
            .map_err(|detail| Error::Upload { detail });
    };
    let manifest = transport::manifest(&api.client, &asset.url).await?;
    mineral_log::debug!(target: "apple_music", stage = "manifest", segments = manifest.segments.len(), elapsed_ms = started.elapsed().as_millis(), "playback preparation");
    let initialization = async {
        let bytes = transport::download(&api.client, &manifest.initialization).await?;
        Ok::<_, Error>(AudioInitialization::parse(&bytes)?)
    };
    let (initialization, key) = tokio::try_join!(
        initialization,
        transport::content_key(&api, &track_id, &manifest.key_uri),
    )?;
    mineral_log::debug!(target: "apple_music", stage = "metadata_and_license", elapsed_ms = started.elapsed().as_millis(), "playback preparation");
    if initialization
        .key_id()
        .is_some_and(|expected| key.id.as_slice() != expected)
    {
        return Err(EncryptionError::KeyIdMismatch.into());
    }
    let key: [u8; 16] =
        key.bytes
            .as_slice()
            .try_into()
            .map_err(|_wrong_length| Error::InvalidMedia {
                field: "AES-128 content key",
            })?;
    let cancellation = options.cancellation().child_token();
    // Only consumers retain this guard. Background tasks cannot keep a dropped
    // playback/export session alive; open failures cancel their work as well.
    let lifetime = Arc::new(cancellation.clone().drop_guard());
    let store = MediaStore::new(
        manifest.segments,
        initialization,
        Zeroizing::new(key),
        api.client.clone(),
        cancellation.clone(),
        *api.config.fragment_header_prefetch_bytes(),
        *api.config.media_concurrency(),
    )
    .await?;
    store.startup(*api.config.startup_buffer()).await?;
    store.acquire();
    mineral_log::info!(target: "apple_music", song_id = %info.song_id.qualified(), elapsed_ms = started.elapsed().as_millis(), "contiguous startup audio ready");
    let control = ReadControl::new(cancellation.clone());
    let packets = {
        let store = store.clone();
        let control = control.clone();
        let lifetime = lifetime.clone();
        tokio::task::spawn_blocking(move || AudioPackets::new(store, control, lifetime)).await??
    };
    let reader = store.reader(Some(control.clone()), lifetime.clone());
    let media = OpenedMedia::new(
        Box::new(reader),
        None,
        info,
        Some(store.transfer.clone()),
        cancellation,
    )
    .with_read_control(control)
    .with_audio_packets(Box::new(packets));
    Ok(match options.capture_target() {
        Some(target) => media.with_capture(store.capture(target, lifetime)),
        None => media,
    })
}
