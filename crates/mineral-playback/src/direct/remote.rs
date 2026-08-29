//! HTTP byte acquisition adapted to a synchronous buffered reader.

use std::fmt::{Debug, Display};
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use color_eyre::eyre::{WrapErr, bail, eyre};
use futures_util::{Stream, TryStreamExt};
use reqwest::header::{self, HeaderValue};
use stream_download::source::{DecodeError, SourceStream};
use stream_download::storage::StorageProvider;
use stream_download::storage::temp::TempStorageProvider;
use stream_download::{Settings, StreamDownload, StreamHandle, StreamPhase, StreamState};
use tokio_util::sync::CancellationToken;

use super::storage::CaptureStorageProvider;
use crate::{
    CaptureReceipt, CaptureTarget, CapturedMedia, MediaReader, RemoteLocator, TransferState,
};

/// A buffered remote reader plus its transfer facts.
pub(super) struct OpenedRemote {
    /// Synchronous stream-download reader.
    pub(super) reader: Box<dyn MediaReader>,

    /// Remote encoded byte length when known.
    pub(super) byte_len: Option<u64>,

    /// Shared transfer progress.
    pub(super) transfer: TransferState,

    /// Producer-owned capture completion when a target was requested.
    pub(super) capture: Option<CaptureReceipt>,
}

/// Type-erased stream-download reader with its producer lifecycle handles.
struct BufferedRemote {
    /// Synchronous reader backed by the selected storage provider.
    reader: Box<dyn MediaReader>,

    /// Completion signal from the stream-download producer.
    completion: StreamHandle,

    /// Cancellation handle for the producer task.
    cancellation: CancellationToken,
}

/// Error wrapper satisfying stream-download's external decode-error trait.
struct RemoteError(color_eyre::Report);

impl Debug for RemoteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.0, formatter)
    }
}

impl Display for RemoteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl std::error::Error for RemoteError {}

impl DecodeError for RemoteError {}

/// A reopenable HTTP source consumed by stream-download.
struct RemoteStream {
    /// Direct remote access description.
    locator: RemoteLocator,

    /// Reusable HTTP client carrying required default headers.
    client: reqwest::Client,

    /// Current response byte stream.
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, RemoteError>> + Send + Sync>>,

    /// Full resource byte length when known.
    content_length: Option<u64>,
}

impl RemoteStream {
    /// Opens an HTTP response range.
    ///
    /// # Params:
    ///   - `client`: Reusable configured HTTP client.
    ///   - `locator`: Remote access description.
    ///   - `start`: Inclusive range start.
    ///   - `end`: Inclusive range end when bounded.
    ///
    /// # Return:
    ///   Response stream and full resource length when available.
    async fn open_range(
        client: &reqwest::Client,
        locator: &RemoteLocator,
        start: u64,
        end: Option<u64>,
    ) -> color_eyre::Result<(
        Pin<Box<dyn Stream<Item = Result<Bytes, RemoteError>> + Send + Sync>>,
        Option<u64>,
    )> {
        let mut request = client.get(locator.url().clone());
        let ranged = start > 0 || end.is_some();
        if ranged {
            request = request.header(
                header::RANGE,
                format!(
                    "bytes={start}-{}",
                    end.map(|value| value.to_string()).unwrap_or_default()
                ),
            );
        }
        let response = request
            .send()
            .await
            .wrap_err_with(|| format!("open media {}", locator.url()))?
            .error_for_status()
            .wrap_err_with(|| format!("media response {}", locator.url()))?;
        let content_length = if ranged {
            response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(parse_content_range_total)
        } else {
            response.content_length()
        };
        let stream = response
            .bytes_stream()
            .map_err(|error| RemoteError(eyre!("http media body: {error}")));
        Ok((Box::pin(stream), content_length))
    }
}

impl SourceStream for RemoteStream {
    type Params = RemoteLocator;

    type StreamCreationError = RemoteError;

    async fn create(locator: Self::Params) -> Result<Self, Self::StreamCreationError> {
        let client = client_with_headers(locator.headers()).map_err(RemoteError)?;
        let (stream, content_length) = Self::open_range(&client, &locator, 0, None)
            .await
            .map_err(RemoteError)?;
        Ok(Self {
            locator,
            client,
            stream,
            content_length,
        })
    }

    fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    async fn seek_range(&mut self, start: u64, end: Option<u64>) -> std::io::Result<()> {
        if Some(start) == self.content_length {
            self.stream = Box::pin(futures_util::stream::empty());
            return Ok(());
        }
        let (stream, content_length) = Self::open_range(&self.client, &self.locator, start, end)
            .await
            .map_err(std::io::Error::other)?;
        self.stream = stream;
        if self.content_length.is_none() {
            self.content_length = content_length;
        }
        Ok(())
    }

    async fn reconnect(&mut self, current_position: u64) -> std::io::Result<()> {
        self.seek_range(current_position, None).await
    }

    fn supports_seek(&self) -> bool {
        true
    }
}

impl Stream for RemoteStream {
    type Item = Result<Bytes, RemoteError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(context)
    }
}

/// Opens a remote locator into a buffered synchronous reader.
///
/// # Params:
///   - `locator`: Remote access description.
///   - `prefetch_bytes`: Bytes to prepare before returning.
///   - `capture_target`: Optional persistent destination for the prepared media.
///   - `cancellation`: Playback-instance cancellation root.
///
/// # Return:
///   Buffered reader, length, and transfer state.
pub(super) async fn open_remote(
    locator: RemoteLocator,
    prefetch_bytes: u64,
    capture_target: Option<CaptureTarget>,
    cancellation: CancellationToken,
) -> color_eyre::Result<OpenedRemote> {
    let stream = RemoteStream::create(locator)
        .await
        .map_err(|error| eyre!("open remote media: {error}"))?;
    let byte_len = stream.content_length();
    let transfer = TransferState::new(byte_len);
    let (buffered, capture) = match capture_target {
        Some(target) => {
            let buffered = buffer_with_storage(
                stream,
                CaptureStorageProvider::new(target.path().to_path_buf()),
                prefetch_bytes,
                &transfer,
            )
            .await?;
            let receipt = CaptureReceipt::new({
                let completion = buffered.completion.clone();
                let transfer = transfer.clone();
                async move {
                    completion.wait_for_completion().await;
                    let path = target.path().to_path_buf();
                    let result = verify_capture(path.clone(), byte_len, &transfer).await;
                    if result.is_err()
                        && let Err(error) = tokio::fs::remove_file(&path).await
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        mineral_log::warn!(
                            target: "playback",
                            path = %path.display(),
                            error = mineral_log::chain(&error),
                            "清理未完成 capture 失败"
                        );
                    }
                    result
                }
            });
            (buffered, Some(receipt))
        }
        None => (
            buffer_with_storage(
                stream,
                TempStorageProvider::new(),
                prefetch_bytes,
                &transfer,
            )
            .await?,
            None,
        ),
    };
    let BufferedRemote {
        reader,
        cancellation: producer_cancellation,
        ..
    } = buffered;
    let producer_finished = producer_cancellation.clone();
    tokio::spawn(async move {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => producer_cancellation.cancel(),
            () = producer_finished.cancelled() => {}
        }
    });
    Ok(OpenedRemote {
        reader,
        byte_len,
        transfer,
        capture,
    })
}

/// Starts stream-download with one concrete storage provider.
///
/// # Params:
///   - `stream`: Reopenable remote byte source.
///   - `storage`: Temporary or persistent producer storage.
///   - `prefetch_bytes`: Bytes to prepare before returning.
///   - `transfer`: Shared producer progress.
///
/// # Return:
///   Type-erased reader plus producer completion and cancellation handles.
async fn buffer_with_storage<P>(
    stream: RemoteStream,
    storage: P,
    prefetch_bytes: u64,
    transfer: &TransferState,
) -> color_eyre::Result<BufferedRemote>
where
    P: StorageProvider + 'static,
    P::Reader: Read + Seek + Send + Sync + 'static,
{
    let observed = transfer.clone();
    let settings = Settings::default()
        .prefetch_bytes(prefetch_bytes)
        .on_progress(move |_stream: &RemoteStream, state: StreamState, _cancel| {
            observed.set_downloaded(state.current_position);
            if state.phase == StreamPhase::Complete {
                observed.mark_complete();
            }
        });
    let reader = StreamDownload::from_stream(stream, storage, settings)
        .await
        .map_err(|error| eyre!("buffer remote media: {error}"))?;
    Ok(BufferedRemote {
        completion: reader.handle(),
        cancellation: reader.get_cancellation_token(),
        reader: Box::new(reader),
    })
}

/// Verifies that stream-download reached its successful terminal phase and filled the target.
///
/// # Params:
///   - `path`: Persistent capture path.
///   - `expected_len`: Full encoded byte length when known.
///   - `transfer`: Producer progress containing the successful terminal phase.
///
/// # Return:
///   Verified capture and its byte length.
///
/// # Error:
///   Returns when the producer stopped early, metadata is unavailable, or the file is truncated.
async fn verify_capture(
    path: PathBuf,
    expected_len: Option<u64>,
    transfer: &TransferState,
) -> color_eyre::Result<CapturedMedia> {
    if !transfer.snapshot().complete {
        bail!("capture producer ended before download completed");
    }
    let bytes = tokio::fs::metadata(&path)
        .await
        .wrap_err_with(|| format!("read capture metadata {}", path.display()))?
        .len();
    if let Some(expected) = expected_len
        && bytes < expected
    {
        bail!("capture truncated: {bytes} / {expected} bytes");
    }
    Ok(CapturedMedia::new(path, bytes))
}

/// Builds an HTTP client carrying direct-media request headers.
///
/// # Params:
///   - `headers`: Request header name/value pairs.
///
/// # Return:
///   A reusable configured client.
fn client_with_headers(headers: &[(String, String)]) -> color_eyre::Result<reqwest::Client> {
    use reqwest::header::{HeaderMap, HeaderName};

    let mut map = HeaderMap::new();
    for (name, value) in headers {
        match (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            (Ok(name), Ok(value)) => {
                map.append(name, value);
            }
            _ => {
                mineral_log::warn!(target: "playback", header = %name, "skip invalid media header")
            }
        }
    }
    reqwest::Client::builder()
        .default_headers(map)
        .build()
        .map_err(|error| eyre!("build media client: {error}"))
}

/// Parses the total length from a Content-Range response header.
///
/// # Params:
///   - `value`: Content-Range header value.
fn parse_content_range_total(value: &HeaderValue) -> Option<u64> {
    let value = value.to_str().ok()?;
    let (_, total) = value.rsplit_once('/')?;
    total.trim().parse().ok()
}
