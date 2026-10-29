//! Catalog artist details and a separate, metadata-only album listing.

use mineral_channel_core::{Page, PageResult};
use mineral_model::{Album, Artist, ArtistId};
use url::Url;

use super::{MetadataClient, mapping};
use crate::{Error, Result};

impl MetadataClient {
    /// Artist attributes and the first top-songs view share one catalog request.
    pub(crate) async fn artist_detail(&self, id: &ArtistId) -> Result<Artist> {
        let path = self.artist_catalog_path(id).await?;
        let mut url = Self::resource_url(&path)?;
        url.query_pairs_mut()
            .append_pair("views", "top-songs")
            .append_pair("include[songs]", "artists,albums");
        let resource = self
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
    pub(crate) async fn artist_albums(
        &self,
        id: &ArtistId,
        page: Page,
    ) -> Result<PageResult<Album>> {
        let path = self.artist_catalog_path(id).await?;
        let mut url = Self::resource_url(&format!("{path}/albums"))?;
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
            let response = self.page(url.as_str()).await?;
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

    /// Library artist links need a catalog counterpart before requesting catalog views.
    async fn artist_catalog_path(&self, id: &ArtistId) -> Result<String> {
        if !id.as_str().starts_with("r.") {
            return self.resource_path("artists", id.as_str()).await;
        }
        let library = self.resource("artists", id.as_str()).await?;
        let catalog = library
            .relationships
            .catalog
            .into_iter()
            .flat_map(|page| page.data)
            .find(|resource| resource.kind == "artists")
            .ok_or(Error::NoCatalogArtist)?;
        self.resource_path("artists", &catalog.id).await
    }
}

fn album_page_url(path: &str, remaining: u32) -> Result<Url> {
    let mut url = MetadataClient::resource_url(path)?;
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
