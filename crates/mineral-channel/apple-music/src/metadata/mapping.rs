//! Projects resource-specific Apple metadata into shared models.

use mineral_model::{
    Album, AlbumId, AlbumRef, Artist, ArtistId, ArtistRef, Playlist, PlaylistId, Song, SongId,
    SourceKind,
};

use super::data::{
    AlbumResource, ArtistResource, PlaylistResource, SongAttributes, SongResource, TrackResource,
};
use crate::{Error, Result};

pub(super) fn song(resource: &SongResource) -> Song {
    let recording = resource.recording();
    song_metadata(
        recording.id.as_str(),
        recording.attributes.as_ref(),
        &recording.artists,
        recording.album.as_ref(),
    )
}

fn song_metadata(
    id: &str,
    attributes: Option<&SongAttributes>,
    artists: &[ArtistRef],
    album: Option<&AlbumRef>,
) -> Song {
    let id = SongId::new(SourceKind::APPLE_MUSIC, id);
    let Some(attributes) = attributes else {
        let name = id.as_str().to_owned();
        return Song::builder().id(id).name(name).unavailable(true).build();
    };
    Song::builder()
        .id(id)
        .name(attributes.name.clone())
        .artists(artists.to_vec())
        .album(album.cloned())
        .duration_ms(attributes.duration_ms)
        .cover_url(attributes.cover.clone())
        .unavailable(!attributes.playable)
        .build()
}

pub(super) fn track(resource: &TrackResource) -> Song {
    match resource {
        TrackResource::Song(resource) => song(resource),
        TrackResource::Unavailable(resource) => {
            let mut song = song_metadata(
                &resource.id,
                resource.attributes.as_ref(),
                &resource.artists,
                resource.album.as_ref(),
            );
            song.unavailable = true;
            song
        }
    }
}

/// Saved albums retain their own identity even when artist metadata comes from the catalog.
pub(super) fn album(resource: &AlbumResource) -> Result<Album> {
    let attributes = resource.attributes.as_ref().ok_or(Error::InvalidData {
        field: "album attributes",
    })?;
    Ok(Album::builder()
        .id(AlbumId::new(SourceKind::APPLE_MUSIC, resource.id.as_str()))
        .name(attributes.name.clone())
        .artists(resource.artists.clone())
        .company(attributes.record_label.clone())
        .track_count(attributes.track_count)
        .cover_url(attributes.cover.clone())
        .publish_time_ms(attributes.publish_time_ms)
        .build())
}

pub(super) fn playlist(resource: &PlaylistResource) -> Result<Playlist> {
    let attributes = resource.attributes.as_ref().ok_or(Error::InvalidData {
        field: "playlist attributes",
    })?;
    let track_count = attributes.track_count.ok_or(Error::InvalidData {
        field: "playlist track total",
    })?;
    let mut playlist = Playlist::builder()
        .id(PlaylistId::new(
            SourceKind::APPLE_MUSIC,
            resource.id.as_str(),
        ))
        .name(attributes.name.clone())
        .cover_url(attributes.cover.clone())
        .track_count(track_count)
        .build();
    if let Some(description) = &attributes.description {
        playlist.description.clone_from(description);
    }
    Ok(playlist)
}

pub(super) fn artist(resource: &ArtistResource) -> Result<Artist> {
    let attributes = resource.attributes.as_ref().ok_or(Error::InvalidData {
        field: "artist attributes",
    })?;
    let mut artist = Artist::builder()
        .id(ArtistId::new(SourceKind::APPLE_MUSIC, resource.id.as_str()))
        .name(attributes.name.clone())
        .avatar_url(attributes.cover.clone())
        .build();
    if let Some(description) = &attributes.description {
        artist.description.clone_from(description);
    }
    Ok(artist)
}
