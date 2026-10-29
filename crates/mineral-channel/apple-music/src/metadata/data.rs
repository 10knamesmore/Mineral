//! Resource identities, metadata and pagination after Apple wire decoding.

use mineral_model::{AlbumRef, ArtistRef, MediaUrl};
use url::Url;

use crate::{Error, Result};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResourceKind {
    Song,

    Album,

    Artist,

    Playlist,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResourceScope {
    Catalog,

    Library,
}

/// The same recording can have separate catalog and personal-library identities.
#[derive(Clone)]
pub(super) enum ResourceId {
    Catalog(String),

    Library(String),
}

impl ResourceId {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Catalog(id) | Self::Library(id) => id,
        }
    }

    pub fn scope(&self) -> ResourceScope {
        match self {
            Self::Catalog(_) => ResourceScope::Catalog,
            Self::Library(_) => ResourceScope::Library,
        }
    }
}

/// Parsed continuation on the Apple metadata origin. Offset is used for playlist membership.
#[derive(Clone)]
pub(super) struct Continuation {
    pub url: Url,

    pub offset: Option<u64>,
}

impl Continuation {
    pub fn offset(&self) -> Result<u64> {
        self.offset.ok_or(Error::InvalidData {
            field: "playlist continuation offset",
        })
    }
}

pub(super) enum PageCursor {
    First,

    Next(Continuation),
}

#[derive(Clone)]
pub(super) struct ResourcePage<T> {
    pub data: Vec<T>,

    pub next: Option<Continuation>,

    pub total: Option<u64>,
}

#[derive(Clone)]
pub(super) struct SongResource {
    pub id: ResourceId,

    /// Relationship references and unavailable members may carry only an identity.
    pub attributes: Option<SongAttributes>,

    /// Library uploads need not have a catalog counterpart.
    pub catalog: Option<Box<Self>>,

    pub artists: Vec<ArtistRef>,

    pub album: Option<AlbumRef>,
}

#[derive(Clone)]
pub(super) struct SongAttributes {
    pub name: String,

    pub cover: Option<MediaUrl>,

    pub duration_ms: Option<u64>,

    pub playable: bool,
}

#[derive(Clone)]
pub(super) struct AlbumResource {
    pub id: ResourceId,

    pub attributes: Option<AlbumAttributes>,

    pub artists: Vec<ArtistRef>,

    pub tracks: Option<ResourcePage<TrackResource>>,
}

#[derive(Clone)]
pub(super) struct AlbumAttributes {
    pub name: String,

    pub cover: Option<MediaUrl>,

    pub track_count: Option<u64>,

    pub publish_time_ms: Option<i64>,

    pub record_label: Option<String>,
}

#[derive(Clone)]
pub(super) struct ArtistResource {
    pub id: ResourceId,

    pub attributes: Option<ArtistAttributes>,

    pub catalog_id: Option<String>,

    pub top_songs: Option<ResourcePage<TrackResource>>,
}

#[derive(Clone)]
pub(super) struct ArtistAttributes {
    pub name: String,

    pub cover: Option<MediaUrl>,

    pub description: Option<String>,
}

#[derive(Clone)]
pub(super) struct PlaylistResource {
    pub id: ResourceId,

    pub attributes: Option<PlaylistAttributes>,

    pub tracks: Option<ResourcePage<TrackResource>>,
}

#[derive(Clone)]
pub(super) struct PlaylistAttributes {
    pub name: String,

    pub cover: Option<MediaUrl>,

    pub description: Option<String>,

    pub track_count: Option<u64>,
}

/// Non-song members keep their positions and display metadata without becoming playable.
#[derive(Clone)]
pub(super) enum TrackResource {
    Song(SongResource),

    Unavailable(UnavailableTrack),
}

#[derive(Clone)]
pub(super) struct UnavailableTrack {
    pub id: String,

    pub attributes: Option<SongAttributes>,

    pub artists: Vec<ArtistRef>,

    pub album: Option<AlbumRef>,
}

impl SongResource {
    pub fn recording(&self) -> &Self {
        match self.catalog.as_deref() {
            Some(catalog) if catalog.attributes.is_some() => catalog,
            _ => self,
        }
    }
}
