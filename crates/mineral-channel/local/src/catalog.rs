//! In-memory songs and playlists produced by one filesystem scan.

use std::path::PathBuf;

use mineral_model::{Playlist, PlaylistEntry, PlaylistId, Song, SongId};
use rustc_hash::FxHashMap;

use crate::metadata::Metadata;

/// Complete library for this channel instance.
#[derive(Default)]
pub(crate) struct Catalog {
    /// Current files indexed by their path-derived song identity.
    pub files: FxHashMap<SongId, FileRecord>,

    /// Current ordered playlists, referring to identities rather than duplicating metadata.
    pub playlists: Vec<LocalPlaylist>,
}

/// File facts and the song projected from them.
pub(crate) struct FileRecord {
    /// Latest absolute path.
    pub path: PathBuf,

    /// Parsed file facts used for playback and lyrics.
    pub metadata: Metadata,

    /// Song projected from this scan, identified by its path digest.
    pub song: Song,
}

/// Songs from one directory, ordered by file path.
pub(crate) struct LocalPlaylist {
    /// Namespaced digest of the canonical directory path.
    pub id: PlaylistId,

    /// Display name.
    pub name: String,

    /// Songs directly contained in this directory.
    pub songs: Vec<SongId>,
}

impl Catalog {
    /// Look up a song in this scan's catalog.
    pub fn file(&self, id: &SongId) -> Option<&FileRecord> {
        self.files.get(id)
    }

    /// Materialize a common playlist from one consistent catalog snapshot.
    pub fn playlist(&self, playlist: &LocalPlaylist, include_songs: bool) -> Playlist {
        let entries = if include_songs {
            PlaylistEntry::enumerate(
                playlist
                    .songs
                    .iter()
                    .filter_map(|id| self.file(id).map(|file| file.song.clone()))
                    .collect(),
            )
        } else {
            Vec::new()
        };
        Playlist::builder()
            .id(playlist.id.clone())
            .name(playlist.name.clone())
            .description(String::new())
            .track_count(playlist.songs.len().try_into().unwrap_or(u64::MAX))
            .cover_url(
                playlist
                    .songs
                    .iter()
                    .find_map(|id| self.file(id).and_then(|file| file.song.cover_url.clone())),
            )
            .entries(entries)
            .build()
    }
}
