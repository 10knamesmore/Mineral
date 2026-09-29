//! Build songs and directory playlists from one completed filesystem scan.

use std::{collections::BTreeMap, path::PathBuf};

use mineral_model::{AlbumRef, ArtistId, ArtistRef, MediaUrl, Song, SongId, SourceKind};

use super::{context::ScanContext, identity};
use crate::{
    catalog::{Catalog, FileRecord, LocalPlaylist},
    error::{Error, Result},
    metadata,
};

/// Include every recognized file and group songs by their directly containing directory.
pub(crate) fn build(context: ScanContext) -> Result<Catalog> {
    let mut files = context.probes.into_iter().collect::<Vec<_>>();
    files.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut catalog = Catalog::default();
    let mut by_directory = BTreeMap::<PathBuf, Vec<SongId>>::new();
    for (path, metadata) in files {
        let directory = path
            .parent()
            .ok_or_else(|| Error::MissingParent { path: path.clone() })?
            .to_path_buf();
        let id = identity::song(&path);
        let artists = metadata
            .artists
            .iter()
            .cloned()
            .map(|name| ArtistRef {
                id: ArtistId::new(SourceKind::LOCAL, &name),
                name,
            })
            .collect();
        let album = metadata
            .album
            .as_ref()
            .map(|name| -> Result<_> {
                Ok(AlbumRef {
                    id: identity::album(name, &metadata.album_artists).map_err(|source| {
                        Error::NumberOverflow {
                            field: "album identity field length",
                            path: path.clone(),
                            source,
                        }
                    })?,
                    name: name.clone(),
                })
            })
            .transpose()?;
        let song = Song::builder()
            .id(id.clone())
            .name(metadata.name.clone())
            .artists(artists)
            .album(album)
            .duration_ms(
                metadata
                    .duration_ms()
                    .map_err(|source| Error::NumberOverflow {
                        field: "song duration in milliseconds",
                        path: path.clone(),
                        source,
                    })?,
            )
            .cover_url(metadata::cover(&path, &metadata).map(MediaUrl::Local))
            .source_url(Some(MediaUrl::Local(path.clone())))
            .build();
        catalog.files.insert(
            id.clone(),
            FileRecord {
                path,
                metadata,
                song,
            },
        );
        by_directory.entry(directory).or_default().push(id);
    }

    catalog.playlists = by_directory
        .into_iter()
        .map(|(directory, songs)| {
            let name = directory
                .file_name()
                .unwrap_or_else(|| directory.as_os_str())
                .to_string_lossy()
                .into_owned();
            LocalPlaylist {
                id: identity::playlist(&directory),
                name,
                songs,
            }
        })
        .collect();
    Ok(catalog)
}
