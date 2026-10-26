//! Projects Apple identities and metadata into shared model types.

use chrono::NaiveDate;
use html2text::config::with_decorator;
use html2text::render::TrivialDecorator;
use mineral_model::{
    Album, AlbumId, AlbumRef, Artist, ArtistId, ArtistRef, MediaUrl, Song, SongId, SourceKind,
};

use crate::wire::{Attributes, Resource};
use crate::{Error, Result};

/// Catalog equivalents unify recording identity; uploads retain their library identity.
pub(crate) fn song_resource(resource: &Resource) -> &Resource {
    resource
        .relationships
        .catalog
        .as_ref()
        .and_then(|page| page.data.first())
        .filter(|catalog| catalog.kind == "songs" && catalog.attributes.is_some())
        .unwrap_or(resource)
}

/// Only expanded artists have a usable identity and name; display names are not IDs.
fn artists(resource: &Resource) -> Vec<ArtistRef> {
    resource
        .relationships
        .artists
        .as_ref()
        .into_iter()
        .flat_map(|page| &page.data)
        .filter_map(|artist| {
            artist.attributes.as_ref().map(|attributes| ArtistRef {
                id: ArtistId::new(SourceKind::APPLE_MUSIC, &artist.id),
                name: attributes.name.clone(),
            })
        })
        .collect()
}

fn cover(attributes: &Attributes) -> Result<Option<MediaUrl>> {
    attributes
        .artwork
        .as_ref()
        .map(|artwork| {
            MediaUrl::remote(&artwork.url.replace("{w}", "600").replace("{h}", "600"))
                .map_err(Error::Url)
        })
        .transpose()
}

/// Missing attributes retain the original identity as an unavailable membership.
pub(crate) fn song(resource: &Resource) -> Result<Song> {
    let resource = song_resource(resource);
    let id = SongId::new(SourceKind::APPLE_MUSIC, &resource.id);
    let Some(attributes) = &resource.attributes else {
        return Ok(Song::builder()
            .id(id)
            .name(resource.id.clone())
            .unavailable(true)
            .build());
    };
    let album = resource
        .relationships
        .albums
        .as_ref()
        .and_then(|page| page.data.first())
        .and_then(|album| {
            album.attributes.as_ref().map(|attributes| AlbumRef {
                id: AlbumId::new(SourceKind::APPLE_MUSIC, &album.id),
                name: attributes.name.clone(),
            })
        });
    Ok(Song::builder()
        .id(id)
        .name(attributes.name.clone())
        .artists(artists(resource))
        .album(album)
        .duration_ms(attributes.duration_in_millis)
        .cover_url(cover(attributes)?)
        .unavailable(attributes.play_params.is_none())
        .build())
}

/// Saved albums keep their library ID even when catalog metadata is expanded.
pub(crate) fn album(resource: &Resource) -> Result<Album> {
    let attributes = resource.attributes.as_ref().ok_or(Error::InvalidData {
        field: "album attributes",
    })?;
    let artist_resource = resource
        .relationships
        .catalog
        .as_ref()
        .and_then(|page| page.data.first())
        .unwrap_or(resource);
    let mut album = Album::builder()
        .id(AlbumId::new(SourceKind::APPLE_MUSIC, &resource.id))
        .name(attributes.name.clone())
        .artists(artists(artist_resource))
        .company(attributes.record_label.clone())
        .track_count(attributes.track_count)
        .cover_url(cover(attributes)?)
        .build();
    if let Some(date) = &attributes.release_date {
        // Apple also supplies YYYY; the timestamp-only model uses that year's start.
        let date = if date.len() == 4 {
            format!("{date}-01-01")
        } else {
            date.clone()
        };
        let date = NaiveDate::parse_from_str(&date, "%Y-%m-%d")?;
        let midnight = date.and_hms_opt(0, 0, 0).ok_or(Error::InvalidData {
            field: "album release date",
        })?;
        album.publish_time_ms = Some(midnight.and_utc().timestamp_millis());
    }
    Ok(album)
}

pub(crate) fn artist(resource: &Resource) -> Result<Artist> {
    let attributes = resource.attributes.as_ref().ok_or(Error::InvalidData {
        field: "artist attributes",
    })?;
    let mut artist = Artist::builder()
        .id(ArtistId::new(SourceKind::APPLE_MUSIC, &resource.id))
        .name(attributes.name.clone())
        .avatar_url(cover(attributes)?)
        .build();
    if let Some(notes) = &attributes.editorial_notes
        && let Some(html) = notes.standard.as_deref().or(notes.short.as_deref())
    {
        // The source length leaves room for the text; the TUI owns display wrapping.
        artist.description = with_decorator(TrivialDecorator::new())
            .string_from_read(html.as_bytes(), html.len())?
            .trim()
            .to_owned();
    }
    Ok(artist)
}
