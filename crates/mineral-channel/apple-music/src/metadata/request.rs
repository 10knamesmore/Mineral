//! Encodes metadata operations and resolves Apple's continuation links at the HTTP boundary.

use mineral_channel_core::Page;
use url::Url;

use super::data::{Continuation, ResourceId, ResourceKind, ResourceScope};
use crate::{Error, Result};

const API_ORIGIN: &str = "https://amp-api.music.apple.com";

impl TryFrom<String> for Continuation {
    type Error = Error;

    fn try_from(path: String) -> Result<Self> {
        let base = Url::parse(API_ORIGIN)?;
        let url = base.join(&path)?;
        if url.origin() != base.origin() || !url.path().starts_with("/v1/") {
            return Err(Error::InvalidData {
                field: "pagination origin",
            });
        }
        let offset = url
            .query_pairs()
            .find(|(name, _)| name == "offset")
            .and_then(|(_, offset)| offset.parse().ok());
        Ok(Self { url, offset })
    }
}

#[derive(Clone, Copy)]
pub(super) enum SongRelationships {
    Artists,

    ArtistsAndAlbums,
}

#[derive(Clone, Copy)]
pub(super) enum PageKind {
    LibraryAlbums,

    LibraryPlaylists,

    AlbumTracks(ResourceScope),

    PlaylistTracks(ResourceScope),

    ArtistAlbums,

    Search(ResourceKind),
}

pub(super) enum Request<'a> {
    Resource {
        kind: ResourceKind,

        id: &'a ResourceId,
    },

    LibraryAlbums,

    LibraryPlaylists,

    ArtistDetail {
        id: &'a str,
    },

    ArtistAlbums {
        id: &'a str,

        page: Page,
    },

    Search {
        kind: ResourceKind,

        query: &'a str,

        page: Page,
    },

    CatalogSongs {
        ids: &'a [&'a str],

        relationships: SongRelationships,
    },

    CatalogAlbums {
        ids: &'a [&'a str],
    },

    Lyrics {
        id: &'a str,

        translation_language: Option<&'a str>,

        romanization_script: Option<&'a str>,
    },

    Next {
        continuation: &'a Continuation,

        kind: PageKind,

        limit: Option<u32>,
    },
}

/// Model IDs carry no Apple address space; classify and validate them once on entry.
pub(super) fn resource_id(kind: ResourceKind, value: &str) -> Result<ResourceId> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Err(Error::InvalidData {
            field: "resource identity",
        });
    }
    let library_prefix = match kind {
        ResourceKind::Song => "i.",
        ResourceKind::Album => "l.",
        ResourceKind::Artist => "r.",
        ResourceKind::Playlist => "p.",
    };
    Ok(if value.starts_with(library_prefix) {
        ResourceId::Library(value.to_owned())
    } else {
        ResourceId::Catalog(value.to_owned())
    })
}

impl ResourceKind {
    pub(super) fn wire_name(self) -> &'static str {
        match self {
            Self::Song => "songs",
            Self::Album => "albums",
            Self::Artist => "artists",
            Self::Playlist => "playlists",
        }
    }
}

impl Request<'_> {
    pub fn url(&self, storefront: &str) -> Result<Url> {
        let base = Url::parse(API_ORIGIN)?;
        let catalog = |kind: ResourceKind| format!("/v1/catalog/{storefront}/{}", kind.wire_name());
        let mut defaults = Vec::new();
        let mut limit = None;
        let mut url = match self {
            Self::Resource { kind, id } => {
                let path = match id {
                    ResourceId::Catalog(id) => format!("{}/{id}", catalog(*kind)),
                    ResourceId::Library(id) => {
                        format!("/v1/me/library/{}/{id}", kind.wire_name())
                    }
                };
                resource_defaults(*kind, id.scope(), &mut defaults);
                base.join(&path)?
            }
            Self::LibraryAlbums => {
                page_defaults(PageKind::LibraryAlbums, &mut defaults);
                base.join("/v1/me/library/albums")?
            }
            Self::LibraryPlaylists => {
                page_defaults(PageKind::LibraryPlaylists, &mut defaults);
                base.join("/v1/me/library/playlists")?
            }
            Self::ArtistDetail { id } => {
                defaults.extend([("views", "top-songs"), ("include[songs]", "artists,albums")]);
                base.join(&format!("{}/{id}", catalog(ResourceKind::Artist)))?
            }
            Self::ArtistAlbums { id, page } => {
                let mut url =
                    base.join(&format!("{}/{id}/albums", catalog(ResourceKind::Artist)))?;
                url.query_pairs_mut()
                    .append_pair("offset", &page.offset.to_string());
                page_defaults(PageKind::ArtistAlbums, &mut defaults);
                limit = Some(page.limit.min(100));
                url
            }
            Self::Search { kind, query, page } => {
                let mut url = base.join(&format!("/v1/catalog/{storefront}/search"))?;
                url.query_pairs_mut()
                    .append_pair("term", query)
                    .append_pair("types", kind.wire_name())
                    .append_pair("offset", &page.offset.to_string());
                page_defaults(PageKind::Search(*kind), &mut defaults);
                limit = Some(page.limit.min(25));
                url
            }
            Self::CatalogSongs { ids, relationships } => {
                let mut url = base.join(&catalog(ResourceKind::Song))?;
                let include = match relationships {
                    SongRelationships::Artists => "artists",
                    SongRelationships::ArtistsAndAlbums => "artists,albums",
                };
                url.query_pairs_mut()
                    .append_pair("ids", &ids.join(","))
                    .append_pair("include", include);
                url
            }
            Self::CatalogAlbums { ids } => {
                let mut url = base.join(&catalog(ResourceKind::Album))?;
                url.query_pairs_mut()
                    .append_pair("ids", &ids.join(","))
                    .append_pair("include", "artists");
                url
            }
            Self::Lyrics {
                id,
                translation_language,
                romanization_script,
            } => {
                if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(Error::InvalidData {
                        field: "lyrics catalog song identity",
                    });
                }
                let mut url = base.join(&format!("{}/{id}", catalog(ResourceKind::Song)))?;
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
                drop(query);
                url
            }
            Self::Next {
                continuation,
                kind,
                limit: requested_limit,
            } => {
                page_defaults(*kind, &mut defaults);
                limit = *requested_limit;
                let mut url = continuation.url.clone();
                if matches!(kind, PageKind::ArtistAlbums) {
                    let pairs: Vec<_> = url
                        .query_pairs()
                        .filter(|(key, _)| {
                            !matches!(
                                key.as_ref(),
                                "include" | "fields[artists]" | "omit[resource]"
                            )
                        })
                        .map(|(key, value)| (key.into_owned(), value.into_owned()))
                        .collect();
                    url.query_pairs_mut().clear().extend_pairs(pairs);
                }
                url
            }
        };
        if let Some(limit) = limit {
            let pairs: Vec<_> = url
                .query_pairs()
                .filter(|(key, _)| key != "limit")
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            url.query_pairs_mut()
                .clear()
                .extend_pairs(pairs)
                .append_pair("limit", &limit.to_string());
        }
        let keys: Vec<_> = url.query_pairs().map(|(key, _)| key.into_owned()).collect();
        for (key, value) in defaults {
            if !keys.iter().any(|existing| existing == key) {
                url.query_pairs_mut().append_pair(key, value);
            }
        }
        Ok(url)
    }
}

fn resource_defaults(
    kind: ResourceKind,
    scope: ResourceScope,
    query: &mut Vec<(&'static str, &'static str)>,
) {
    match (scope, kind) {
        (ResourceScope::Library, ResourceKind::Album) => {
            query.push(("include", "catalog,tracks"));
            library_album_defaults(query);
        }
        (ResourceScope::Library, ResourceKind::Playlist) => {
            query.extend([
                ("include", "tracks"),
                ("limit[tracks]", "100"),
                ("include[library-songs]", "catalog"),
            ]);
            library_defaults(query);
        }
        (ResourceScope::Library, _) => {
            query.push(("include", "catalog"));
            library_defaults(query);
        }
        (ResourceScope::Catalog, ResourceKind::Song) => {
            query.push(("include", "artists,albums"));
        }
        (ResourceScope::Catalog, ResourceKind::Album) => {
            query.push(("include", "artists"));
        }
        (ResourceScope::Catalog, ResourceKind::Playlist) => query.extend([
            ("include", "tracks"),
            ("limit[tracks]", "100"),
            ("extend[playlists]", "trackCount"),
            ("include[songs]", "artists,albums"),
        ]),
        (ResourceScope::Catalog, ResourceKind::Artist) => {}
    }
}

fn page_defaults(kind: PageKind, query: &mut Vec<(&'static str, &'static str)>) {
    match kind {
        PageKind::LibraryAlbums | PageKind::PlaylistTracks(ResourceScope::Library) => {
            query.push(("include", "catalog"));
            library_defaults(query);
        }
        PageKind::LibraryPlaylists => {
            query.extend([("include", "tracks"), ("limit[tracks]", "1")]);
            library_defaults(query);
        }
        PageKind::AlbumTracks(ResourceScope::Library) => {
            query.push(("include", "catalog"));
            library_album_defaults(query);
        }
        PageKind::ArtistAlbums => query.extend([
            ("include", "artists"),
            ("fields[artists]", "name"),
            ("omit[resource]", "autos"),
        ]),
        PageKind::Search(ResourceKind::Playlist) => {
            query.push(("extend[playlists]", "trackCount"));
        }
        PageKind::Search(_)
        | PageKind::AlbumTracks(ResourceScope::Catalog)
        | PageKind::PlaylistTracks(ResourceScope::Catalog) => {}
    }
    if !matches!(kind, PageKind::Search(_)) {
        query.push(("limit", "100"));
    }
}

fn library_defaults(query: &mut Vec<(&'static str, &'static str)>) {
    query.extend([
        ("include[songs]", "artists,albums"),
        ("include[albums]", "artists"),
    ]);
}

fn library_album_defaults(query: &mut Vec<(&'static str, &'static str)>) {
    query.extend([
        ("include[songs]", "artists"),
        ("fields[artists]", "name"),
        ("omit[resource]", "autos"),
        ("include[albums]", "artists"),
    ]);
}
