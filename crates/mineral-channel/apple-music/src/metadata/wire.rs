//! Decodes Apple's resource envelopes into resource-specific metadata before loading and mapping.

use chrono::NaiveDate;
use html2text::config::with_decorator;
use html2text::render::TrivialDecorator;
use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef, MediaUrl, SourceKind};
use rustc_hash::FxHashMap;
use serde::Deserialize;

use super::data::{
    AlbumAttributes, AlbumResource, ArtistAttributes, ArtistResource, PlaylistAttributes,
    PlaylistResource, ResourceId, ResourceKind, ResourcePage, SongAttributes, SongResource,
    TrackResource, UnavailableTrack,
};
use crate::{Error, Result};

pub(super) trait DecodeResource: Sized {
    fn decode(resource: Resource) -> Result<Self>;
}

pub(super) trait MetadataResource: DecodeResource {
    const KIND: ResourceKind;
}

#[derive(Deserialize)]
pub(super) struct SearchResponse {
    results: FxHashMap<String, PageResponse>,
}

impl SearchResponse {
    pub fn take<T: MetadataResource>(&mut self) -> Result<Option<ResourcePage<T>>> {
        self.results
            .remove(T::KIND.wire_name())
            .map(PageResponse::decode)
            .transpose()
    }
}

#[derive(Deserialize)]
pub(super) struct PageResponse {
    data: Vec<Resource>,

    next: Option<String>,

    meta: Option<PageMeta>,
}

impl PageResponse {
    pub fn decode<T: DecodeResource>(self) -> Result<ResourcePage<T>> {
        Ok(ResourcePage {
            data: self
                .data
                .into_iter()
                .map(T::decode)
                .collect::<Result<_>>()?,
            next: self.next.map(TryInto::try_into).transpose()?,
            total: self.meta.and_then(|meta| meta.total),
        })
    }
}

#[derive(Deserialize)]
struct PageMeta {
    total: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ResourceType {
    Songs,

    LibrarySongs,

    Albums,

    LibraryAlbums,

    Artists,

    LibraryArtists,

    Playlists,

    LibraryPlaylists,

    #[serde(other)]
    Other,
}

impl ResourceType {
    fn identity(self, kind: ResourceKind, id: String) -> Result<ResourceId> {
        match (self, kind) {
            (Self::Songs, ResourceKind::Song)
            | (Self::Albums, ResourceKind::Album)
            | (Self::Artists, ResourceKind::Artist)
            | (Self::Playlists, ResourceKind::Playlist) => Ok(ResourceId::Catalog(id)),
            (Self::LibrarySongs, ResourceKind::Song)
            | (Self::LibraryAlbums, ResourceKind::Album)
            | (Self::LibraryArtists, ResourceKind::Artist)
            | (Self::LibraryPlaylists, ResourceKind::Playlist) => Ok(ResourceId::Library(id)),
            _ => Err(Error::InvalidData {
                field: "metadata resource type",
            }),
        }
    }
}

#[derive(Deserialize)]
pub(super) struct Resource {
    id: String,

    #[serde(rename = "type")]
    kind: ResourceType,

    attributes: Option<Attributes>,

    #[serde(default)]
    relationships: Relationships,

    #[serde(default)]
    views: Views,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Views {
    top_songs: Option<PageResponse>,
}

#[derive(Default, Deserialize)]
struct Relationships {
    tracks: Option<PageResponse>,

    catalog: Option<PageResponse>,

    artists: Option<PageResponse>,

    albums: Option<PageResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attributes {
    name: String,

    artwork: Option<Artwork>,

    duration_in_millis: Option<u64>,

    track_count: Option<u64>,

    play_params: Option<PlayParams>,

    release_date: Option<String>,

    record_label: Option<String>,

    editorial_notes: Option<EditorialNotes>,

    description: Option<Description>,
}

#[derive(Deserialize)]
struct Description {
    standard: String,
}

#[derive(Deserialize)]
struct EditorialNotes {
    standard: Option<String>,

    short: Option<String>,
}

#[derive(Deserialize)]
struct PlayParams {}

#[derive(Deserialize)]
struct Artwork {
    url: String,
}

fn cover(artwork: Option<Artwork>) -> Result<Option<MediaUrl>> {
    artwork
        .map(|artwork| {
            MediaUrl::remote(&artwork.url.replace("{w}", "600").replace("{h}", "600"))
                .map_err(Error::Url)
        })
        .transpose()
}

fn artist_references(page: Option<PageResponse>) -> Vec<ArtistRef> {
    page.into_iter()
        .flat_map(|page| page.data)
        .filter_map(|resource| {
            resource.attributes.map(|attributes| ArtistRef {
                id: ArtistId::new(SourceKind::APPLE_MUSIC, resource.id),
                name: attributes.name,
            })
        })
        .collect()
}

fn album_reference(page: Option<PageResponse>) -> Option<AlbumRef> {
    page.into_iter()
        .flat_map(|page| page.data)
        .next()
        .and_then(|resource| {
            resource.attributes.map(|attributes| AlbumRef {
                id: AlbumId::new(SourceKind::APPLE_MUSIC, resource.id),
                name: attributes.name,
            })
        })
}

fn catalog<T: DecodeResource>(
    page: Option<PageResponse>,
    kind: ResourceType,
) -> Result<Option<Box<T>>> {
    page.into_iter()
        .flat_map(|page| page.data)
        .find(|resource| resource.kind == kind)
        .map(|resource| T::decode(resource).map(Box::new))
        .transpose()
}

fn song_attributes(attributes: Attributes) -> Result<SongAttributes> {
    Ok(SongAttributes {
        name: attributes.name,
        cover: cover(attributes.artwork)?,
        duration_ms: attributes.duration_in_millis,
        playable: attributes.play_params.is_some(),
    })
}

impl MetadataResource for SongResource {
    const KIND: ResourceKind = ResourceKind::Song;
}

impl DecodeResource for SongResource {
    fn decode(resource: Resource) -> Result<Self> {
        Ok(Self {
            id: resource.kind.identity(Self::KIND, resource.id)?,
            attributes: resource.attributes.map(song_attributes).transpose()?,
            catalog: catalog(resource.relationships.catalog, ResourceType::Songs)?,
            artists: artist_references(resource.relationships.artists),
            album: album_reference(resource.relationships.albums),
        })
    }
}

impl MetadataResource for AlbumResource {
    const KIND: ResourceKind = ResourceKind::Album;
}

impl DecodeResource for AlbumResource {
    fn decode(resource: Resource) -> Result<Self> {
        let attributes = resource
            .attributes
            .map(|attributes| {
                Ok::<_, Error>(AlbumAttributes {
                    name: attributes.name,
                    cover: cover(attributes.artwork)?,
                    track_count: attributes.track_count,
                    publish_time_ms: attributes.release_date.map(release_date).transpose()?,
                    record_label: attributes.record_label,
                })
            })
            .transpose()?;
        let catalog = resource
            .relationships
            .catalog
            .into_iter()
            .flat_map(|page| page.data)
            .find(|resource| resource.kind == ResourceType::Albums);
        let artists = match catalog {
            Some(catalog) => catalog.relationships.artists,
            None => resource.relationships.artists,
        };
        Ok(Self {
            id: resource.kind.identity(Self::KIND, resource.id)?,
            attributes,
            artists: artist_references(artists),
            tracks: resource
                .relationships
                .tracks
                .map(PageResponse::decode)
                .transpose()?,
        })
    }
}

fn release_date(value: String) -> Result<i64> {
    // Apple supplies year, month or day precision; timestamps use the reported period's start.
    let value = match value.len() {
        4 => format!("{value}-01-01"),
        7 => format!("{value}-01"),
        _ => value,
    };
    let date = NaiveDate::parse_from_str(&value, "%Y-%m-%d")?;
    let midnight = date.and_hms_opt(0, 0, 0).ok_or(Error::InvalidData {
        field: "album release date",
    })?;
    Ok(midnight.and_utc().timestamp_millis())
}

impl MetadataResource for ArtistResource {
    const KIND: ResourceKind = ResourceKind::Artist;
}

impl DecodeResource for ArtistResource {
    fn decode(resource: Resource) -> Result<Self> {
        let attributes = resource
            .attributes
            .map(|attributes| {
                let description = attributes
                    .editorial_notes
                    .and_then(|notes| notes.standard.or(notes.short))
                    .map(|html| {
                        with_decorator(TrivialDecorator::new())
                            .string_from_read(html.as_bytes(), html.len())
                            .map(|text| text.trim().to_owned())
                    })
                    .transpose()?;
                Ok::<_, Error>(ArtistAttributes {
                    name: attributes.name,
                    cover: cover(attributes.artwork)?,
                    description,
                })
            })
            .transpose()?;
        Ok(Self {
            id: resource.kind.identity(Self::KIND, resource.id)?,
            attributes,
            catalog_id: resource
                .relationships
                .catalog
                .into_iter()
                .flat_map(|page| page.data)
                .find(|resource| resource.kind == ResourceType::Artists)
                .map(|resource| resource.id),
            top_songs: resource
                .views
                .top_songs
                .map(PageResponse::decode)
                .transpose()?,
        })
    }
}

impl MetadataResource for PlaylistResource {
    const KIND: ResourceKind = ResourceKind::Playlist;
}

impl DecodeResource for PlaylistResource {
    fn decode(resource: Resource) -> Result<Self> {
        let id = resource.kind.identity(Self::KIND, resource.id)?;
        let tracks = resource
            .relationships
            .tracks
            .map(PageResponse::decode)
            .transpose()?;
        let attributes = resource
            .attributes
            .map(|attributes| {
                let track_count = match &id {
                    ResourceId::Catalog(_) => attributes.track_count,
                    ResourceId::Library(_) => tracks.as_ref().and_then(|page| page.total),
                };
                Ok::<_, Error>(PlaylistAttributes {
                    name: attributes.name,
                    cover: cover(attributes.artwork)?,
                    description: attributes
                        .description
                        .map(|description| description.standard),
                    track_count,
                })
            })
            .transpose()?;
        Ok(Self {
            id,
            attributes,
            tracks,
        })
    }
}

impl DecodeResource for TrackResource {
    fn decode(resource: Resource) -> Result<Self> {
        if matches!(
            resource.kind,
            ResourceType::Songs | ResourceType::LibrarySongs
        ) {
            return SongResource::decode(resource).map(Self::Song);
        }
        let catalog = catalog::<SongResource>(resource.relationships.catalog, ResourceType::Songs)?;
        let unavailable = match catalog {
            Some(song) if song.attributes.is_some() => UnavailableTrack {
                id: song.id.as_str().to_owned(),
                attributes: song.attributes,
                artists: song.artists,
                album: song.album,
            },
            _ => UnavailableTrack {
                id: resource.id,
                attributes: resource.attributes.map(song_attributes).transpose()?,
                artists: artist_references(resource.relationships.artists),
                album: album_reference(resource.relationships.albums),
            },
        };
        Ok(Self::Unavailable(unavailable))
    }
}
