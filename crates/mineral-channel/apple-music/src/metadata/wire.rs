//! Apple metadata response envelopes, private to resource loading and model mapping.

use rustc_hash::FxHashMap;
use serde::Deserialize;

/// Apple search groups each requested resource type under results, not top-level data.
#[derive(Deserialize)]
pub(super) struct SearchResponse {
    pub results: FxHashMap<String, ResourcePage>,
}

/// A page retains raw membership order and Apple's continuation URL.
#[derive(Clone, Deserialize)]
pub(super) struct ResourcePage {
    pub data: Vec<Resource>,

    pub next: Option<String>,
}

/// Catalog and library identities remain distinct until explicitly mapped.
#[derive(Clone, Deserialize)]
pub(super) struct Resource {
    pub id: String,

    #[serde(rename = "type")]
    pub kind: String,

    pub attributes: Option<Attributes>,

    #[serde(default)]
    pub relationships: Relationships,

    #[serde(default)]
    pub views: Views,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct Views {
    pub top_songs: Option<ResourcePage>,
}

#[derive(Clone, Default, Deserialize)]
pub(super) struct Relationships {
    pub tracks: Option<ResourcePage>,

    pub catalog: Option<ResourcePage>,

    pub artists: Option<ResourcePage>,

    pub albums: Option<ResourcePage>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Attributes {
    pub name: String,

    pub artwork: Option<Artwork>,

    pub duration_in_millis: Option<u64>,

    pub track_count: Option<u64>,

    pub play_params: Option<PlayParams>,

    pub release_date: Option<String>,

    pub record_label: Option<String>,

    pub editorial_notes: Option<EditorialNotes>,
}

#[derive(Clone, Deserialize)]
pub(super) struct EditorialNotes {
    pub standard: Option<String>,

    pub short: Option<String>,
}

/// Presence indicates a playback identity is available.
#[derive(Clone, Deserialize)]
pub(super) struct PlayParams {}

#[derive(Clone, Deserialize)]
pub(super) struct Artwork {
    /// Template containing {w} and {h} placeholders.
    pub url: String,
}
