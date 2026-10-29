//! Fetches one catalog song's lyric representations and maps Apple TTML into shared lyrics.

use mineral_model::{Lyrics, SongId};
use url::Url;

use super::ttml;
use super::wire::SongLyricsResponse;
use crate::metadata::MetadataClient;
use crate::{Error, Result};

impl MetadataClient {
    /// Prefer syllable lyrics; absent representations return empty lyrics, malformed TTML fails.
    pub(crate) async fn lyrics(&self, id: &SongId) -> Result<Lyrics> {
        let id = id.as_str();
        let catalog_id = if id.starts_with("i.") {
            let resource = self.resource("songs", id).await?;
            let Some(catalog) = resource.relationships.catalog.and_then(|page| {
                page.data
                    .into_iter()
                    .find(|resource| resource.kind == "songs")
            }) else {
                // Uploaded library recordings need not have a catalog equivalent.
                return Ok(Lyrics::default());
            };
            catalog.id
        } else {
            id.to_owned()
        };
        if catalog_id.is_empty() || !catalog_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::InvalidData {
                field: "lyrics catalog song identity",
            });
        }
        let account = self.account.account().await?;
        let mut url = Url::parse(&format!(
            "https://amp-api.music.apple.com/v1/catalog/{}/songs/{catalog_id}",
            account.storefront
        ))?;
        let translation_language = self.lyrics_translation_language.as_deref();
        let romanization_script = self.lyrics_romanization_script.as_deref();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("include", "lyrics,syllable-lyrics")
                .append_pair("extend", "ttmlLocalizations");
            if let Some(language) = translation_language {
                query.append_pair("l[lyrics]", language);
            }
            if let Some(script) = romanization_script {
                query.append_pair("l[script]", script);
            }
        }
        let response: SongLyricsResponse = self
            .account
            .authenticated_json(|| self.account.http().get(url.clone()), |_| false)
            .await?;
        let song = response
            .data
            .into_iter()
            .next()
            .ok_or(Error::Http { status: 404 })?;
        let Some(relationships) = song.relationships else {
            return Ok(Lyrics::default());
        };
        let representation = relationships
            .syllable_lyrics
            .into_iter()
            .flat_map(|page| page.data)
            .next()
            .or_else(|| {
                relationships
                    .lyrics
                    .into_iter()
                    .flat_map(|page| page.data)
                    .next()
            });
        let Some(resource) = representation else {
            return Ok(Lyrics::default());
        };
        let attributes = resource.attributes.ok_or(Error::InvalidData {
            field: "lyrics attributes",
        })?;
        let document =
            attributes
                .ttml
                .or(attributes.ttml_localizations)
                .ok_or(Error::InvalidData {
                    field: "lyrics TTML",
                })?;
        ttml::parse(&document, translation_language, romanization_script)
    }
}
