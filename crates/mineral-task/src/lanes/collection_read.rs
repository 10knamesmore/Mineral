use std::sync::Arc;

use mineral_channel_core::store::{LibraryStore, NamespaceStore};
use mineral_channel_core::{Error, MusicChannel, PlaylistDetail, PlaylistLoad, Result};
use mineral_model::{
    Album, AlbumId, CachePolicy, PlaylistEntry, PlaylistId, Song, SongId, SourceKind,
};
use rustc_hash::{FxHashMap, FxHashSet};

/// Reads local membership through its owner and fresh metadata through each song's source.
pub(super) struct CollectionReader {
    library: Option<Arc<dyn LibraryStore>>,

    channels: FxHashMap<SourceKind, Arc<dyn MusicChannel>>,

    stores: FxHashMap<SourceKind, Arc<dyn NamespaceStore>>,
}

impl CollectionReader {
    pub(super) fn new(
        channels: &[Arc<dyn MusicChannel>],
        stores: &[Arc<dyn NamespaceStore>],
        library: Option<Arc<dyn LibraryStore>>,
    ) -> Self {
        Self {
            library,
            channels: channels
                .iter()
                .map(|channel| (channel.source(), channel.clone()))
                .collect(),
            stores: stores
                .iter()
                .map(|store| (store.source(), store.clone()))
                .collect(),
        }
    }

    pub(super) async fn playlist(
        &self,
        channel: &dyn MusicChannel,
        id: &PlaylistId,
        load: PlaylistLoad,
        cache: CachePolicy,
    ) -> Result<PlaylistDetail> {
        let mut detail = channel.playlist_detail(id, load, cache).await?;
        if cache == CachePolicy::Reload {
            if id.namespace() == SourceKind::MINERAL {
                let library = self.library.as_ref().ok_or(Error::NotSupported)?;
                let entries =
                    library
                        .playlist_entries(id)
                        .await
                        .map_err(|source| Error::Storage {
                            source: Box::new(source),
                        })?;
                let mut groups = FxHashMap::<SourceKind, FxHashSet<SongId>>::default();
                for entry in &entries {
                    groups
                        .entry(entry.song_id.namespace())
                        .or_default()
                        .insert(entry.song_id.clone());
                }
                let mut fresh = FxHashMap::default();
                for (source, ids) in groups {
                    let channel = self.channels.get(&source).ok_or(Error::NotSupported)?;
                    let ids = ids.into_iter().collect::<Vec<_>>();
                    let songs = channel.songs_detail(&ids, cache).await?;
                    fresh.extend(songs.into_iter().map(|song| (song.id.clone(), song)));
                    if ids.iter().any(|id| !fresh.contains_key(id)) {
                        return Err(Error::NotFound);
                    }
                }
                // Replace only metadata: relation positions, duplicate entries and membership stay local.
                detail.playlist.entries = entries
                    .into_iter()
                    .map(|entry| {
                        Ok(PlaylistEntry::builder()
                            .index(entry.index)
                            .song(fresh.get(&entry.song_id).cloned().ok_or(Error::NotFound)?)
                            .build())
                    })
                    .collect::<Result<Vec<_>>>()?;
                detail.playlist.track_count = u64::try_from(detail.playlist.entries.len())
                    .map_err(|source| Error::Parse {
                        source: Box::new(source),
                    })?;
            }
            self.persist(detail.playlist.entries.iter().map(|entry| &entry.song))
                .await?;
        }
        Ok(detail)
    }

    pub(super) async fn album(
        &self,
        channel: &dyn MusicChannel,
        id: &AlbumId,
        cache: CachePolicy,
    ) -> Result<Album> {
        let album = channel.album_detail(id, cache).await?;
        if cache == CachePolicy::Reload {
            self.persist(album.tracks.iter().map(|track| &track.song))
                .await?;
        }
        Ok(album)
    }

    async fn persist<'a>(&self, songs: impl Iterator<Item = &'a Song> + Send) -> Result<()> {
        let mut groups = FxHashMap::<SourceKind, Vec<&Song>>::default();
        for song in songs {
            groups.entry(song.id.namespace()).or_default().push(song);
        }
        for (source, songs) in groups {
            self.stores
                .get(&source)
                .ok_or(Error::NotSupported)?
                .upsert_meta_batch(&songs)
                .await
                .map_err(|source| Error::Storage {
                    source: Box::new(source),
                })?;
        }
        Ok(())
    }
}
