//! Playback boundary contract tests independent of concrete music sources.

use std::io::{Cursor, Read};
use std::sync::Arc;

use async_trait::async_trait;
use mineral_model::{BitRate, PlaybackMediaInfo, SongId, SourceKind};
use mineral_playback::{
    DirectMedia, MediaReader, OpenOptions, OpenedMedia, PlaybackProvider, PlaybackRegistry,
    PlaybackRequest, PreparedPlayback, SeekSupport,
};
use tokio_util::sync::CancellationToken;

/// Fake provider whose raw bytes contain a prefix absent from decoder-ready output.
struct LengthChangingProvider {
    /// Source identity served by this fake.
    source: SourceKind,
}

/// Single-use fake plan carrying media facts into open.
struct StripPrefixPrepared {
    /// Final decoder-ready media facts.
    info: PlaybackMediaInfo,
}

#[async_trait]
impl PlaybackProvider for LengthChangingProvider {
    fn source(&self) -> SourceKind {
        self.source
    }

    async fn resolve(
        &self,
        request: PlaybackRequest,
        _cancellation: CancellationToken,
    ) -> color_eyre::Result<Box<dyn PreparedPlayback>> {
        Ok(Box::new(StripPrefixPrepared {
            info: PlaybackMediaInfo {
                song_id: request.song_id().clone(),
                bitrate_bps: None,
                quality: request.quality(),
                size: None,
                format: None,
                bit_depth: None,
                substituted: false,
            },
        }))
    }
}

#[async_trait]
impl PreparedPlayback for StripPrefixPrepared {
    fn direct_media(&self) -> Option<&DirectMedia> {
        None
    }

    async fn open(self: Box<Self>, options: OpenOptions) -> color_eyre::Result<OpenedMedia> {
        let raw = b"ENCAPSULATED:decoder-ready";
        let prepared = raw
            .strip_prefix(b"ENCAPSULATED:")
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture prefix missing"))?
            .to_vec();
        let byte_len = Some(u64::try_from(prepared.len())?);
        let reader: Box<dyn MediaReader> = Box::new(Cursor::new(prepared));
        Ok(OpenedMedia::new(
            reader,
            SeekSupport::RandomAccess,
            byte_len,
            self.info,
            None,
            options.cancellation().clone(),
        ))
    }
}

/// Builds one fake provider handle for registry tests.
fn provider(source: SourceKind) -> Arc<dyn PlaybackProvider> {
    Arc::new(LengthChangingProvider { source })
}

/// A provider without direct media may change encoded length before consumers read it.
#[tokio::test]
async fn preparation_may_change_length_without_direct_media() -> color_eyre::Result<()> {
    let source = SourceKind::from_static("prepared-test", "Prepared test");
    let registry = PlaybackRegistry::new(vec![provider(source)])?;
    let provider = registry
        .get(source)
        .ok_or_else(|| color_eyre::eyre::eyre!("provider missing"))?;
    let cancellation = CancellationToken::new();
    let prepared = provider
        .resolve(
            PlaybackRequest::new(SongId::new(source, "song"), BitRate::Lossless),
            cancellation.clone(),
        )
        .await?;
    assert!(prepared.direct_media().is_none());
    let opened = prepared
        .open(OpenOptions::new(cancellation, /*prefetch_bytes*/ 0))
        .await?;
    assert_eq!(opened.byte_len(), Some(13));
    let mut output = Vec::new();
    opened.into_reader().read_to_end(&mut output)?;
    assert_eq!(output, b"decoder-ready");
    Ok(())
}

/// Duplicate source registration is rejected instead of silently replacing a provider.
#[test]
fn registry_rejects_duplicate_source() {
    let source = SourceKind::from_static("duplicate-test", "Duplicate test");
    assert!(PlaybackRegistry::new(vec![provider(source), provider(source)]).is_err());
}

/// A local direct locator does not alter the song's original source identity.
#[test]
fn direct_locator_does_not_change_source_identity() {
    let info = PlaybackMediaInfo {
        song_id: SongId::new(SourceKind::NETEASE, "42"),
        bitrate_bps: None,
        quality: BitRate::Exhigh,
        size: None,
        format: None,
        bit_depth: None,
        substituted: false,
    };
    let media = DirectMedia::local(info, std::path::PathBuf::from("cached.mp3"));
    assert_eq!(media.info().song_id.namespace(), SourceKind::NETEASE);
}
