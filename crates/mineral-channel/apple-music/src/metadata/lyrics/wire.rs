//! Lyrics attributes do not share the required name field of catalog metadata.

use serde::Deserialize;

use crate::{Error, Result};

#[derive(Deserialize)]
pub(super) struct SongLyricsResponse {
    data: Vec<Song>,
}

impl SongLyricsResponse {
    pub fn document(self) -> Result<Option<String>> {
        let song = self
            .data
            .into_iter()
            .next()
            .ok_or(Error::Http { status: 404 })?;
        let Some(relationships) = song.relationships else {
            return Ok(None);
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
            return Ok(None);
        };
        let attributes = resource.attributes.ok_or(Error::InvalidData {
            field: "lyrics attributes",
        })?;
        attributes
            .ttml
            .or(attributes.ttml_localizations)
            .map(Some)
            .ok_or(Error::InvalidData {
                field: "lyrics TTML",
            })
    }
}

#[derive(Deserialize)]
pub(super) struct Song {
    pub relationships: Option<Relationships>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct Relationships {
    pub lyrics: Option<LyricPage>,

    pub syllable_lyrics: Option<LyricPage>,
}

#[derive(Deserialize)]
pub(super) struct LyricPage {
    pub data: Vec<LyricResource>,
}

#[derive(Deserialize)]
pub(super) struct LyricResource {
    pub attributes: Option<Attributes>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Attributes {
    pub ttml: Option<String>,

    pub ttml_localizations: Option<String>,
}
