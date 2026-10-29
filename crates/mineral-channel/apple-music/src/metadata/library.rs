//! Personal-library membership is read from library endpoints, not inferred from catalog data.

use mineral_model::Album;

use super::data::PageCursor;
use super::request::{PageKind, Request};
use super::{MetadataClient, mapping};
use crate::Result;

impl MetadataClient {
    /// Read every saved-album page before publishing the collection.
    pub(crate) async fn my_albums(&self) -> Result<Vec<Album>> {
        let mut next = Some(PageCursor::First);
        let mut albums = Vec::new();
        while let Some(cursor) = next {
            let request = match &cursor {
                PageCursor::Next(continuation) => Request::Next {
                    continuation,
                    kind: PageKind::LibraryAlbums,
                    limit: None,
                },
                PageCursor::First => Request::LibraryAlbums,
            };
            let page = self.page(request).await?;
            albums.extend(
                page.data
                    .iter()
                    .map(mapping::album)
                    .collect::<Result<Vec<_>>>()?,
            );
            next = page.next.map(PageCursor::Next);
        }
        mineral_log::info!(target: "apple_music", albums = albums.len(), "saved albums loaded");
        Ok(albums)
    }
}
