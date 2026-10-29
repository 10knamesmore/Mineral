//! Acquires media bytes and bounded fragment headers, enforcing CDN range responses.

use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::ops::Range;

use reqwest::header::RANGE;
use reqwest::{Client, Response, StatusCode};
use shiguredo_mp4::boxes::{MdatBox, MoofBox};
use shiguredo_mp4::{BoxHeader, Decode};
use url::Url;

use super::error::{Error, Result};

/// An HLS resource or its absolute, end-exclusive byte range within a CDN object.
#[derive(Clone)]
pub(super) struct MediaResource {
    pub url: Url,

    pub range: Option<Range<usize>>,
}

impl MediaResource {
    /// CENC offsets may be relative to the containing CDN object rather than the response.
    pub(super) fn resource_offset(&self) -> Result<u64> {
        self.range
            .as_ref()
            .map_or(Ok(0), |range| Ok(u64::try_from(range.start)?))
    }
}

pub(super) async fn download(client: &Client, resource: &MediaResource) -> Result<Vec<u8>> {
    let mut request = client.get(resource.url.clone());
    if let Some(range) = &resource.range {
        request = request.header(RANGE, range_header(range)?);
    }
    let response = response(request.send().await.map_err(Error::network)?)?;
    if resource.range.is_some() && response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(Error::ByteRange);
    }
    let bytes = response.bytes().await.map_err(Error::network)?;
    if resource
        .range
        .as_ref()
        .is_some_and(|range| bytes.len() != range.len())
    {
        return Err(Error::ByteRange);
    }
    Ok(bytes.to_vec())
}

/// Acquires only the moof metadata; missing prefix bytes are fetched as exact ranges.
pub(super) async fn fragment_header(
    client: &Client,
    resource: &MediaResource,
    prefetch_bytes: NonZeroUsize,
) -> Result<MoofBox> {
    let prefix = read_header_prefix(client, resource, prefetch_bytes).await?;
    let mut position = 0_usize;
    loop {
        let bytes = header_bytes(client, resource, &prefix, position, BoxHeader::MAX_SIZE).await?;
        let (header, _) = BoxHeader::decode(bytes.as_ref())?;
        let size = usize::try_from(header.box_size.get())?;
        if header.box_type == MoofBox::TYPE {
            let bytes = header_bytes(client, resource, &prefix, position, size).await?;
            return Ok(MoofBox::decode(bytes.as_ref())?.0);
        }
        if size == 0 || header.box_type == MdatBox::TYPE {
            return Err(Error::InvalidMedia {
                field: "fragment header before media",
            });
        }
        position = position.checked_add(size).ok_or(Error::Overflow)?;
    }
}

async fn header_bytes<'a>(
    client: &Client,
    resource: &MediaResource,
    prefix: &'a [u8],
    start: usize,
    length: usize,
) -> Result<Cow<'a, [u8]>> {
    let end = start.checked_add(length).ok_or(Error::Overflow)?;
    match prefix.get(start..end) {
        Some(bytes) => Ok(Cow::Borrowed(bytes)),
        None => Ok(Cow::Owned(
            read_header_range(client, resource, start, length).await?,
        )),
    }
}

async fn read_header_range(
    client: &Client,
    resource: &MediaResource,
    start: usize,
    length: usize,
) -> Result<Vec<u8>> {
    let origin = resource.range.as_ref().map_or(0, |range| range.start);
    let start = origin.checked_add(start).ok_or(Error::Overflow)?;
    let end = start.checked_add(length).ok_or(Error::Overflow)?;
    let response = response(
        client
            .get(resource.url.clone())
            .header(RANGE, range_header(&(start..end))?)
            .send()
            .await
            .map_err(Error::network)?,
    )?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(Error::ByteRange);
    }
    let bytes = response.bytes().await.map_err(Error::network)?;
    if bytes.len() != length {
        return Err(Error::ByteRange);
    }
    Ok(bytes.to_vec())
}

/// Read a bounded fragment prefix; a short fragment may end before the requested limit.
async fn read_header_prefix(
    client: &Client,
    resource: &MediaResource,
    limit: NonZeroUsize,
) -> Result<Vec<u8>> {
    let start = resource.range.as_ref().map_or(0, |range| range.start);
    let length = resource
        .range
        .as_ref()
        .map_or(limit.get(), |range| limit.get().min(range.len()));
    let end = start.checked_add(length).ok_or(Error::Overflow)?;
    let response = response(
        client
            .get(resource.url.clone())
            .header(RANGE, range_header(&(start..end))?)
            .send()
            .await
            .map_err(Error::network)?,
    )?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(Error::ByteRange);
    }
    let bytes = response.bytes().await.map_err(Error::network)?;
    if bytes.len() > length {
        return Err(Error::ByteRange);
    }
    Ok(bytes.to_vec())
}

fn range_header(range: &Range<usize>) -> Result<String> {
    let end = range.end.checked_sub(1).ok_or(Error::ByteRange)?;
    Ok(format!("bytes={}-{end}", range.start))
}

pub(super) fn response(response: Response) -> Result<Response> {
    if !response.status().is_success() {
        return Err(Error::Http {
            status: response.status().as_u16(),
        });
    }
    Ok(response)
}
