//! Personal-library membership is read from library endpoints, not inferred from catalog data.

use mineral_model::Album;

use super::{MetadataClient, mapping};
use crate::Result;

impl MetadataClient {
    /// Read every saved-album page before publishing the collection.
    pub(crate) async fn my_albums(&self) -> Result<Vec<Album>> {
        let mut path = "/v1/me/library/albums".to_owned();
        let mut albums = Vec::new();
        loop {
            let page = self.page(&path).await?;
            albums.extend(
                page.data
                    .iter()
                    .map(mapping::album)
                    .collect::<Result<Vec<_>>>()?,
            );
            let Some(next) = page.next else {
                break;
            };
            path = next;
        }
        mineral_log::info!(target: "apple_music", albums = albums.len(), "saved albums loaded");
        Ok(albums)
    }
}
