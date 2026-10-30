use mineral_model::{CollectionCategory, RefreshScope};

use super::AppState;
use crate::runtime::collection::CollectionId;
use crate::runtime::track_pos::{PendingRestore, TrackPos};

impl AppState {
    /// Keep the opened parent header and cursor identity while discarding its read state.
    pub(crate) fn invalidate_collections(&mut self, scope: &RefreshScope) {
        if let Some(collection) = self.ui.browse.nav.opened_collection.clone()
            && match (&collection, scope) {
                (CollectionId::Playlist(id), RefreshScope::Playlist(parent)) => id == parent,
                (CollectionId::Album(id), RefreshScope::Album(parent)) => id == parent,
                _ => false,
            }
        {
            let scroll = self.ui.browse.tracks.scroll();
            if let Some(entry) = self.filtered_tracks().get(scroll.sel()) {
                self.ui.browse.nav.pending_track_restore = Some(PendingRestore {
                    collection,
                    pos: TrackPos {
                        song_id: entry.data.song.id.clone(),
                        index: scroll.sel(),
                        screen_row: scroll.sel().saturating_sub(scroll.scroll_target()),
                    },
                });
            }
        }
        let library = &mut self.models.library;
        match scope {
            RefreshScope::Collections(CollectionCategory::Playlists) => {
                library.playlists.clear();
                library.tracks.clear();
                library.tracks_requested.clear();
            }
            RefreshScope::Collections(CollectionCategory::Albums) => {
                library.albums.clear();
                library.album_tracks.clear();
                library.albums_requested.clear();
            }
            RefreshScope::Playlist(id) => {
                library.tracks.remove(id);
                library.tracks_requested.remove(id);
                for playlist in &mut library.playlists {
                    if playlist.data.id == *id {
                        playlist.data.entries.clear();
                    }
                }
            }
            RefreshScope::Album(id) => {
                library.album_tracks.remove(id);
                library.albums_requested.remove(id);
                for album in &mut library.albums {
                    if album.data.id == *id {
                        album.data.tracks.clear();
                    }
                }
            }
        }
        library.tracks_generation = library.tracks_generation.next();
        for results in self.ui.channel_search.retained_results_mut() {
            results.detail.invalidate_collections(scope);
        }
        self.ui.browse.invalidate_expansions();
    }
}
