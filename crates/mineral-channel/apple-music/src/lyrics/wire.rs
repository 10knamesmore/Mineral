//! Lyrics attributes do not share the required name field of catalog metadata.

use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct SongLyricsResponse {
    pub data: Vec<Song>,
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
