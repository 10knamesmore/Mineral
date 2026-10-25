//! Catalog artist details and a separate, metadata-only album listing.

use mineral_channel_core::{Page, PageResult};
use mineral_model::{Album, Artist, ArtistId};
use url::Url;

use crate::api::Api;
use crate::{Error, Result, mapping};

/// Artist attributes and the first top-songs view share one catalog request.
pub(crate) async fn detail(api: &Api, id: &ArtistId) -> Result<Artist> {
    let path = catalog_path(api, id).await?;
    let mut url = Api::url(&path)?;
    url.query_pairs_mut()
        .append_pair("views", "top-songs")
        .append_pair("include[songs]", "artists,albums");
    let resource = api
        .page(url.as_str())
        .await?
        .data
        .into_iter()
        .next()
        .ok_or(Error::Http { status: 404 })?;
    let mut artist = mapping::artist(&resource)?;
    artist.id = id.clone();
    if let Some(songs) = resource.views.top_songs {
        artist.songs = songs
            .data
            .iter()
            .map(mapping::song)
            .collect::<Result<_>>()?;
    }
    Ok(artist)
}

/// Follow Apple's page boundary until the requested page is filled or exhausted.
/// Album tracks are not requested; they belong to album_detail.
pub(crate) async fn albums(api: &Api, id: &ArtistId, page: Page) -> Result<PageResult<Album>> {
    let path = catalog_path(api, id).await?;
    let mut url = Api::url(&format!("{path}/albums"))?;
    url.query_pairs_mut()
        .append_pair("offset", &page.offset.to_string());
    let mut next = Some(url.to_string());
    let mut albums = Vec::new();
    while u32::try_from(albums.len())? < page.limit {
        let Some(path) = next.take() else {
            break;
        };
        let remaining = page.limit - u32::try_from(albums.len())?;
        let url = album_page_url(&path, remaining)?;
        let response = api.page(url.as_str()).await?;
        albums.extend(
            response
                .data
                .iter()
                .map(mapping::album)
                .collect::<Result<Vec<_>>>()?,
        );
        next = response.next;
    }
    Ok(PageResult::new(albums, next.is_some()))
}

fn album_page_url(path: &str, remaining: u32) -> Result<Url> {
    let mut url = Api::url(path)?;
    let preserved: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| {
            !matches!(
                key.as_ref(),
                "limit" | "include" | "fields[artists]" | "omit[resource]"
            )
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(preserved)
        .append_pair("limit", &remaining.min(100).to_string())
        .append_pair("include", "artists")
        .append_pair("fields[artists]", "name")
        .append_pair("omit[resource]", "autos");
    Ok(url)
}

async fn catalog_path(api: &Api, id: &ArtistId) -> Result<String> {
    if !id.as_str().starts_with("r.") {
        return api.resource_path("artists", id.as_str()).await;
    }
    let library = api.resource("artists", id.as_str()).await?;
    let catalog = library
        .relationships
        .catalog
        .into_iter()
        .flat_map(|page| page.data)
        .find(|resource| resource.kind == "artists")
        .ok_or(Error::NoCatalogArtist)?;
    api.resource_path("artists", &catalog.id).await
}
