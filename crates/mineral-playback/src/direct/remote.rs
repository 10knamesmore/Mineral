//! HTTP acquisition for a track whose encoded bytes remain in memory.

use std::io;
use std::ops::Range;

use reqwest::header::{self, HeaderValue};
use reqwest::{Client, Response, StatusCode};
use thiserror::Error;
use tokio::{select, spawn};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::buffer::{DownloadBuffer, RemoteReader};
use super::capture::capture;
use crate::{
    CaptureReceipt, CaptureTarget, MediaReader, ReadControl, RemoteLocator,
    Result as PlaybackResult, TransferState,
};

/// Failures while acquiring and placing remote media bytes.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// Request construction, transport, status, or response-body failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    /// The response does not describe the requested bytes of the same resource.
    #[error(
        "invalid media response from {url}: {status}, requested {requested:?}, Content-Range {content_range:?}"
    )]
    InvalidResponse {
        /// Requested media URL.
        url: Box<Url>,

        /// Response status.
        status: StatusCode,

        /// Half-open range; None denotes the initial full-resource request.
        requested: Option<Range<u64>>,

        /// Server-provided range, if present.
        content_range: Option<HeaderValue>,
    },

    /// The body did not match its declared byte count.
    #[error("media response length mismatch: expected {expected}, received {received}")]
    LengthMismatch {
        /// Declared response byte count.
        expected: u64,

        /// Observed response byte count.
        received: u64,
    },

    /// Received bytes could not be placed in the encoded-media buffer.
    #[error("buffer media bytes")]
    Buffer(#[from] io::Error),
}

pub(super) struct OpenedRemote {
    pub(super) reader: Box<dyn MediaReader>,

    pub(super) byte_len: Option<u64>,

    pub(super) transfer: TransferState,

    pub(super) read_control: ReadControl,

    pub(super) capture: Option<CaptureReceipt>,
}

struct HttpMedia {
    locator: RemoteLocator,

    client: Client,
}

impl HttpMedia {
    async fn open_response(
        &self,
        range: Option<Range<u64>>,
        length: Option<u64>,
    ) -> Result<Response, DownloadError> {
        let mut request = self.client.get(self.locator.url().clone());
        for (name, value) in self.locator.headers() {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(range) = &range {
            // Internal ranges are half-open; HTTP's last byte is inclusive.
            request = request.header(
                header::RANGE,
                format!("bytes={}-{}", range.start, range.end - 1),
            );
        }
        let response = request.send().await?.error_for_status()?;
        let status = response.status();
        let content_range = response.headers().get(header::CONTENT_RANGE);
        let valid = match &range {
            Some(requested) => {
                status == StatusCode::PARTIAL_CONTENT
                    && content_range.and_then(parse_content_range).is_some_and(
                        |(received, total)| received == *requested && Some(total) == length,
                    )
            }
            None => status == StatusCode::OK,
        };
        if !valid {
            return Err(DownloadError::InvalidResponse {
                url: Box::new(self.locator.url().clone()),
                status,
                requested: range,
                content_range: content_range.cloned(),
            });
        }
        Ok(response)
    }
}

pub(super) async fn open_remote(
    locator: RemoteLocator,
    prefetch_bytes: u64,
    target: Option<CaptureTarget>,
    cancellation: CancellationToken,
) -> PlaybackResult<OpenedRemote> {
    let http = HttpMedia {
        client: Client::builder().build().map_err(DownloadError::from)?,
        locator,
    };
    let response = http.open_response(None, None).await?;
    let byte_len = response.content_length();
    let cancellation = cancellation.child_token();
    let lifetime = cancellation.clone().drop_guard();
    let buffer = DownloadBuffer::new(byte_len, cancellation.clone());
    let producer = buffer.clone();
    spawn(async move {
        select! {
            biased;
            () = producer.cancellation.cancelled() => {}
            result = download(&http, response, &producer) => producer.finish(result),
        }
    });
    buffer.startup(prefetch_bytes).await?;
    let read_control = ReadControl::new(cancellation);
    let capture = target.map(|target| capture(buffer.clone(), target));
    Ok(OpenedRemote {
        byte_len: buffer.length(),
        transfer: buffer.transfer.clone(),
        reader: Box::new(RemoteReader::new(buffer, read_control.clone(), lifetime)),
        read_control,
        capture,
    })
}

/// One response is active at a time. A new missing read interrupts it; otherwise acquisition
/// continues through its gap, then fills the earliest remaining gap for capture.
async fn download(
    http: &HttpMedia,
    mut response: Response,
    buffer: &DownloadBuffer,
) -> Result<(), DownloadError> {
    let mut start = 0;
    let mut end = buffer.length();
    while let Some(mut range) = receive(http, &mut response, start, end, buffer).await? {
        drop(response);
        loop {
            select! {
                biased;
                () = buffer.demand_changed.notified() => {
                    if let Some(requested) = buffer.requested_gap() {
                        range = requested;
                    }
                }
                opened = http.open_response(Some(range.clone()), buffer.length()) => {
                    response = opened?;
                    start = range.start;
                    end = Some(range.end);
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn receive(
    http: &HttpMedia,
    response: &mut Response,
    start: u64,
    end: Option<u64>,
    buffer: &DownloadBuffer,
) -> Result<Option<Range<u64>>, DownloadError> {
    let mut position = start;
    loop {
        let chunk = select! {
            biased;
            () = buffer.demand_changed.notified() => {
                if let Some(range) = buffer.requested_gap()
                    && range.start != position
                {
                    return Ok(Some(range));
                }
                continue;
            }
            chunk = response.chunk() => chunk.map_err(|error| error.with_url(http.locator.url().clone()))?,
        };
        let Some(bytes) = chunk else {
            if let Some(end) = end {
                if position != end {
                    return Err(DownloadError::LengthMismatch {
                        expected: end - start,
                        received: position - start,
                    });
                }
            } else {
                buffer.set_length(position);
            }
            return Ok(buffer.next_gap());
        };
        let next = position
            .checked_add(u64::try_from(bytes.len()).map_err(io::Error::other)?)
            .ok_or(io::ErrorKind::InvalidData)
            .map_err(io::Error::from)?;
        if let Some(end) = end
            && next > end
        {
            return Err(DownloadError::LengthMismatch {
                expected: end - start,
                received: next - start,
            });
        }
        position = buffer.store_chunk(position, bytes)?;
    }
}

fn parse_content_range(value: &HeaderValue) -> Option<(Range<u64>, u64)> {
    let value = value.to_str().ok()?.strip_prefix("bytes ")?;
    let (bounds, total) = value.split_once('/')?;
    let (start, last) = bounds.split_once('-')?;
    let start = start.parse().ok()?;
    let end = last.parse::<u64>().ok()?.checked_add(1)?;
    Some((start..end, total.parse().ok()?))
}
