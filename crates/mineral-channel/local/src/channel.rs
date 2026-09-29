//! Narrow local channel surface: songs, playlists, lyrics and playback only.

use async_trait::async_trait;
use mineral_channel_core::{
    ArtistSections, ChannelCaps, Error, MusicChannel, PlaylistDetail, PlaylistLoad, Result,
};
use mineral_model::{
    DirectMedia, Lyrics, PlaybackMediaInfo, Playlist, PlaylistId, Song, SongId, SourceKind,
};
use mineral_playback::{
    DirectPreparedPlayback, Error as PlaybackError, PlaybackProvider, PlaybackRequest,
    PreparedPlayback,
};
use tokio_util::sync::CancellationToken;

use crate::{LocalLibrary, error::Error as LocalError};

#[async_trait]
impl MusicChannel for LocalLibrary {
    fn source(&self) -> SourceKind {
        SourceKind::LOCAL
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .artist_sections(ArtistSections::new(Vec::new()))
            .build()
    }

    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>> {
        let catalog = self.catalog().await?;
        ids.iter()
            .map(|id| {
                catalog
                    .file(id)
                    .map(|file| file.song.clone())
                    .ok_or(Error::NotFound)
            })
            .collect()
    }

    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        let catalog = self.catalog().await?;
        Ok(catalog
            .playlists
            .iter()
            .map(|playlist| catalog.playlist(playlist, false))
            .collect())
    }

    async fn playlist_detail(
        &self,
        id: &PlaylistId,
        _load: PlaylistLoad,
    ) -> Result<PlaylistDetail> {
        let catalog = self.catalog().await?;
        let playlist = catalog
            .playlists
            .iter()
            .find(|playlist| &playlist.id == id)
            .ok_or(Error::NotFound)?;
        Ok(PlaylistDetail::complete(catalog.playlist(playlist, true)))
    }

    async fn lyrics(&self, id: &SongId) -> Result<Lyrics> {
        let catalog = self.catalog().await?;
        let record = catalog.file(id).ok_or(Error::NotFound)?;
        let sidecar = record.path.with_extension("lrc");
        let lyrics = match tokio::fs::read(&sidecar).await {
            Ok(bytes) => {
                let text = crate::metadata::decode_sidecar(&bytes).map_err(|source| {
                    LocalError::LyricsEncoding {
                        path: sidecar.clone(),
                        source,
                    }
                })?;
                let lyrics = Lyrics {
                    lines: mineral_model::parse_lrc(&text),
                };
                mineral_log::debug!(target: "local_library", path = %sidecar.display(), lines = lyrics.lines.len(), "sidecar lyrics selected");
                lyrics
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                record.metadata.embedded_lyrics.clone().unwrap_or_default()
            }
            Err(source) => {
                return Err(LocalError::File {
                    operation: "read lyric sidecar",
                    path: sidecar,
                    source,
                }
                .into());
            }
        };
        Ok(lyrics)
    }
}

#[async_trait]
impl PlaybackProvider for LocalLibrary {
    fn source(&self) -> SourceKind {
        SourceKind::LOCAL
    }

    async fn resolve(
        &self,
        request: PlaybackRequest,
        cancellation: CancellationToken,
    ) -> mineral_playback::Result<Box<dyn PreparedPlayback>> {
        if cancellation.is_cancelled() {
            return Err(PlaybackError::Cancelled);
        }
        let catalog = self.catalog().await?;
        if cancellation.is_cancelled() {
            return Err(PlaybackError::Cancelled);
        }
        let file = catalog
            .file(request.song_id())
            .ok_or_else(|| LocalError::SongNotFound {
                id: request.song_id().clone(),
            })?;
        let info = PlaybackMediaInfo {
            song_id: file.song.id.clone(),
            bitrate_bps: file.metadata.bitrate_bps.map(Into::into),
            size: Some(file.metadata.size),
            format: Some(file.metadata.format.clone()),
            bit_depth: file.metadata.bit_depth.map(Into::into),
            substituted: false,
        };
        Ok(DirectPreparedPlayback::boxed(DirectMedia::local(
            info,
            file.path.clone(),
        )))
    }
}
