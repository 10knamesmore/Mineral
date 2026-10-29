//! Lists personal-library playlists and reads library or catalog playlist membership.
//! Each detail request rebuilds its cumulative prefix; no partial playlist cache is retained.

use mineral_channel_core::{PlaylistDetail, PlaylistLoad};
use mineral_model::{CollectionIndex, Playlist, PlaylistEntry, PlaylistId};

use super::{MetadataClient, mapping};
use crate::{Error, Result};

impl MetadataClient {
    /// Read all playlist headers before publishing the collection.
    /// The limited tracks relationship supplies counts, not entries for the list view.
    pub(crate) async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        let mut path = "/v1/me/library/playlists".to_owned();
        let mut playlists = Vec::new();
        loop {
            let page = self.page(&path).await?;
            for resource in page.data {
                playlists.push(mapping::playlist(&resource)?);
            }
            let Some(next) = page.next else {
                break;
            };
            path = next;
        }
        mineral_log::info!(target: "apple_music", playlists = playlists.len(), "library playlists loaded");
        Ok(playlists)
    }

    /// Preview returns the embedded track page; More rereads the prefix through the
    /// requested continuation page. Complete follows every next URL or returns its error.
    pub(crate) async fn playlist_detail(
        &self,
        id: &PlaylistId,
        load: PlaylistLoad,
    ) -> Result<PlaylistDetail> {
        let mut resource = self.resource("playlists", id.as_str()).await?;
        let mut playlist = mapping::playlist(&resource)?;
        let mut page = resource
            .relationships
            .tracks
            .take()
            .ok_or(Error::InvalidData {
                field: "playlist tracks relationship",
            })?;
        let mut offset = 0_u64;
        loop {
            for (position, resource) in page.data.iter().enumerate() {
                let mut song = mapping::song(resource)?;
                // Keep non-song members in their original positions without offering playback.
                if !matches!(resource.kind.as_str(), "songs" | "library-songs") {
                    song.unavailable = true;
                }
                let index =
                    offset
                        .checked_add(u64::try_from(position)?)
                        .ok_or(Error::InvalidData {
                            field: "playlist track index",
                        })?;
                playlist.entries.push(
                    PlaylistEntry::builder()
                        .index(CollectionIndex::new(index))
                        .song(song)
                        .build(),
                );
            }
            let Some(path) = page.next else {
                return Ok(PlaylistDetail::complete(playlist));
            };
            let next_offset = playlist_offset(&path)?;
            if next_offset <= offset {
                return Err(Error::InvalidData {
                    field: "playlist continuation order",
                });
            }
            let requested_page_loaded = match load {
                PlaylistLoad::Preview => true,
                PlaylistLoad::More { offset: requested } => offset >= requested,
                PlaylistLoad::Complete => false,
            };
            if requested_page_loaded {
                return Ok(PlaylistDetail {
                    playlist,
                    complete: false,
                    next_offset: Some(next_offset),
                });
            }
            offset = next_offset;
            page = self.page(&path).await?;
        }
    }
}

/// Apple's offset identifies the next page; mapped song counts cannot supply it.
fn playlist_offset(path: &str) -> Result<u64> {
    MetadataClient::resource_url(path)?
        .query_pairs()
        .find(|(name, _)| name == "offset")
        .and_then(|(_, offset)| offset.parse().ok())
        .ok_or(Error::InvalidData {
            field: "playlist continuation offset",
        })
}
