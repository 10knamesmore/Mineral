//! Fetches one catalog song's lyric representations and maps Apple TTML into shared lyrics.

use super::ttml;
use super::wire::SongLyricsResponse;
use crate::Result;
use crate::metadata::MetadataClient;
use crate::metadata::data::{ResourceId, ResourceKind, SongResource};
use crate::metadata::request::{Request, resource_id};
use mineral_model::{Lyrics, SongId};

impl MetadataClient {
    /// Prefer syllable lyrics; absent representations return empty lyrics, malformed TTML fails.
    pub(crate) async fn lyrics(&self, id: &SongId) -> Result<Lyrics> {
        let address = resource_id(ResourceKind::Song, id.as_str())?;
        let catalog_id = match address {
            ResourceId::Catalog(id) => id,
            library @ ResourceId::Library(_) => {
                let resource: SongResource = self.resource(&library).await?;
                let Some(catalog) = resource.catalog else {
                    return Ok(Lyrics::default());
                };
                catalog.id.as_str().to_owned()
            }
        };
        let translation_language = self.lyrics_translation_language.as_deref();
        let romanization_script = self.lyrics_romanization_script.as_deref();
        let response: SongLyricsResponse = self
            .fetch(Request::Lyrics {
                id: &catalog_id,
                translation_language,
                romanization_script,
            })
            .await?;
        let Some(document) = response.document()? else {
            return Ok(Lyrics::default());
        };
        ttml::parse(&document, translation_language, romanization_script)
    }
}
