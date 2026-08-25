//! Single-use prepared playback for direct remote and local media.

use std::io::BufReader;

use async_trait::async_trait;
use color_eyre::eyre::{WrapErr, eyre};
use mineral_model::StreamLayout;

use super::reader::{CancellationReader, ForwardOnlyReader};
use super::remote::open_remote;
use crate::{
    DirectLocator, DirectMedia, MediaReader, OpenOptions, OpenedMedia, PreparedPlayback,
    SeekSupport,
};

/// Built-in prepared playback for a direct locator.
pub struct DirectPreparedPlayback {
    /// Direct media capability opened when consumed.
    media: DirectMedia,
}

impl DirectPreparedPlayback {
    /// Creates a single-use direct playback plan.
    ///
    /// # Params:
    ///   - `media`: Direct media capability to open.
    #[must_use]
    pub fn new(media: DirectMedia) -> Self {
        Self { media }
    }

    /// Returns this plan as a boxed provider result.
    #[must_use]
    pub fn boxed(media: DirectMedia) -> Box<dyn PreparedPlayback> {
        Box::new(Self::new(media))
    }

    /// Opens a local path as decoder-ready media.
    ///
    /// # Params:
    ///   - `self`: Consumed direct plan.
    ///   - `options`: Open-time execution controls.
    fn open_local(self, options: &OpenOptions) -> color_eyre::Result<OpenedMedia> {
        if options.cancellation().is_cancelled() {
            return Err(eyre!("playback instance cancelled before local open"));
        }
        let path = self
            .media
            .locator()
            .local_path()
            .ok_or_else(|| eyre!("direct media is not local"))?;
        let file = std::fs::File::open(path)
            .wrap_err_with(|| format!("open local media {}", path.display()))?;
        let byte_len = file.metadata().ok().map(|metadata| metadata.len());
        let reader: Box<dyn MediaReader> = Box::new(BufReader::new(file));
        let cancellation = options.cancellation().clone();
        Ok(OpenedMedia::new(
            Box::new(CancellationReader::new(reader, cancellation.clone())),
            SeekSupport::RandomAccess,
            byte_len,
            self.media.info().clone(),
            None,
            cancellation,
        ))
    }

    /// Opens a remote locator as decoder-ready media.
    ///
    /// # Params:
    ///   - `self`: Consumed direct plan.
    ///   - `options`: Open-time execution controls.
    async fn open_remote(self, options: OpenOptions) -> color_eyre::Result<OpenedMedia> {
        let remote = self
            .media
            .locator()
            .remote()
            .cloned()
            .ok_or_else(|| eyre!("direct media is not remote"))?;
        let cancellation = options.cancellation().clone();
        let opened = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                return Err(eyre!("playback instance cancelled during remote open"));
            }
            result = open_remote(remote, options.prefetch_bytes(), cancellation.clone()) => result?,
        };
        let (reader, seek_support, byte_len): (Box<dyn MediaReader>, SeekSupport, Option<u64>) =
            match self.media.layout() {
                StreamLayout::Contiguous => {
                    (opened.reader, SeekSupport::RandomAccess, opened.byte_len)
                }
                StreamLayout::Chunked => (
                    Box::new(ForwardOnlyReader::new(opened.reader)),
                    SeekSupport::ForwardOnly,
                    None,
                ),
            };
        Ok(OpenedMedia::new(
            Box::new(CancellationReader::new(reader, cancellation.clone())),
            seek_support,
            byte_len,
            self.media.info().clone(),
            Some(opened.transfer),
            cancellation,
        ))
    }
}

#[async_trait]
impl PreparedPlayback for DirectPreparedPlayback {
    fn direct_media(&self) -> Option<&DirectMedia> {
        Some(&self.media)
    }

    async fn open(self: Box<Self>, options: OpenOptions) -> color_eyre::Result<OpenedMedia> {
        let is_remote = matches!(self.media.locator(), DirectLocator::Remote(_));
        if is_remote {
            self.open_remote(options).await
        } else {
            self.open_local(&options)
        }
    }
}
