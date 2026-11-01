//! Lists personal-library playlists and reads library or catalog playlist membership.
//! A detail stream retains its cumulative prefix and Apple's continuation URL between polls.

use async_stream::try_stream;
use mineral_channel_core::{FetchSnapshot, FetchStream, PlaylistDetail};
use mineral_model::{CollectionIndex, Playlist, PlaylistEntry, PlaylistId};

use super::data::{PageCursor, PlaylistResource, ResourceKind, ResourceScope, TrackResource};
use super::request::{PageKind, Request, SongRelationships, resource_id};
use super::{MetadataClient, mapping};
use crate::{Error, Result};

impl MetadataClient {
    /// Read all playlist headers before publishing the collection.
    /// The limited tracks relationship supplies counts, not entries for the list view.
    pub(crate) async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        let mut next = Some(PageCursor::First);
        let mut playlists = Vec::new();
        while let Some(cursor) = next {
            let request = match &cursor {
                PageCursor::Next(continuation) => Request::Next {
                    continuation,
                    kind: PageKind::LibraryPlaylists,
                    limit: None,
                },
                PageCursor::First => Request::LibraryPlaylists,
            };
            let page = self.page(request).await?;
            for resource in page.data {
                playlists.push(mapping::playlist(&resource)?);
            }
            next = page.next.map(PageCursor::Next);
        }
        mineral_log::info!(target: "apple_music", playlists = playlists.len(), "library playlists loaded");
        Ok(playlists)
    }

    /// Each poll reads one track page and publishes the accumulated membership.
    pub(crate) fn playlist_detail<'a>(
        &'a self,
        id: &'a PlaylistId,
    ) -> FetchStream<'a, PlaylistDetail> {
        Box::pin(try_stream! {
            let address = resource_id(ResourceKind::Playlist, id.as_str())?;
            let mut resource: PlaylistResource = self.resource(&address).await?;
            let mut playlist = mapping::playlist(&resource)?;
            let mut page = resource.tracks.take().ok_or(Error::InvalidData {
                field: "playlist tracks relationship",
            })?;
            let mut offset = 0_u64;
            loop {
                for (position, resource) in page.data.iter().enumerate() {
                    let song = mapping::track(resource);
                    let index = offset.checked_add(u64::try_from(position).map_err(Error::from)?)
                        .ok_or(Error::InvalidData { field: "playlist track index" })?;
                    playlist.entries.push(
                        PlaylistEntry::builder()
                            .index(CollectionIndex::new(index))
                            .song(song)
                            .build(),
                    );
                }
                let Some(continuation) = page.next else {
                    yield FetchSnapshot::Fresh(PlaylistDetail::complete(playlist));
                    break;
                };
                let next_offset = continuation.offset()?;
                if next_offset <= offset {
                    Err(Error::InvalidData { field: "playlist continuation order" })?;
                }
                yield FetchSnapshot::Fresh(PlaylistDetail { playlist: playlist.clone(), complete: false });
                offset = next_offset;
                page = self.page(Request::Next {
                    continuation: &continuation,
                    kind: PageKind::PlaylistTracks(address.scope()),
                    limit: None,
                }).await?;
                if address.scope() == ResourceScope::Catalog {
                    self.hydrate_songs(
                        page.data.iter_mut().filter_map(|track| match track {
                            TrackResource::Song(song) => Some(song),
                            TrackResource::Unavailable(_) => None,
                        }),
                        SongRelationships::ArtistsAndAlbums,
                    ).await?;
                }
            }
        })
    }
}
