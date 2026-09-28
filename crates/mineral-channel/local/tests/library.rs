//! Real files and SQLite verify the public local-library boundary.

use std::path::Path;

use color_eyre::eyre::eyre;
use mineral_channel_core::{MusicChannel, PlaylistLoad};
use mineral_channel_local::LocalLibrary;
use mineral_model::{BitRate, Playlist, Song, SongId, SourceKind};
use mineral_persist::ServerStore;
use mineral_playback::{OpenOptions, PlaybackProvider, PlaybackRequest};
use rustc_hash::FxHashSet;
use tokio_util::sync::CancellationToken;

/// Opening another channel instance models a daemon restart without a reload API.
fn open(directory: &Path, store: &ServerStore) -> LocalLibrary {
    LocalLibrary::new(
        store.clone(),
        directory.join("covers"),
        vec![directory.join("music")],
    )
}

/// Fully materialize the local playlists through the channel interface.
async fn playlists(library: &LocalLibrary) -> color_eyre::Result<Vec<Playlist>> {
    let mut result = Vec::new();
    for playlist in library.my_playlists().await? {
        let detail = library
            .playlist_detail(&playlist.id, PlaylistLoad::Complete)
            .await?;
        assert!(detail.complete);
        assert!(detail.next_offset.is_none());
        result.push(detail.playlist);
    }
    Ok(result)
}

/// Find a fixture by its title, with useful diagnostics instead of indexing assumptions.
fn named(playlists: &[Playlist], name: &str) -> color_eyre::Result<Song> {
    playlists
        .iter()
        .flat_map(|playlist| &playlist.entries)
        .find(|entry| entry.song.name == name)
        .map(|entry| entry.song.clone())
        .ok_or_else(|| eyre!("missing fixture {name}"))
}

/// A small real PCM file exercises Lofty and the existing local media opener.
fn audio(path: &Path) -> color_eyre::Result<()> {
    mineral_test::write_wav(path, &vec![100; 4_800], 1, 48_000)
}

/// A path retains identity across tag edits; moving and copying produce new identities.
#[tokio::test]
async fn one_time_load_identity_lyrics_and_playback() -> color_eyre::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("music");
    std::fs::create_dir_all(root.join("album"))?;
    audio(&root.join("first.wav"))?;
    // A recognized extension must not override the actual container format.
    audio(&root.join("album/second.aac"))?;
    tokio::fs::write(root.join("notes.txt"), "not audio").await?;
    std::os::unix::fs::symlink(&root, root.join("cycle"))?;
    let store = ServerStore::open(&temp.path().join("mineral.db")).await?;
    let mut library = open(temp.path(), &store);
    let (first_read, concurrent_read) = tokio::join!(playlists(&library), playlists(&library));
    let first_read = first_read?;
    assert_eq!(first_read, concurrent_read?);
    assert_eq!(first_read.len(), 2);
    assert_eq!(first_read.iter().map(|p| p.entries.len()).sum::<usize>(), 2);
    let first = named(&playlists(&library).await?, "first")?;
    assert_eq!(first.id.value().len(), 64);
    assert!(
        first
            .id
            .value()
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    for playlist in &first_read {
        assert_eq!(playlist.id.value().len(), 64);
        assert!(
            playlist
                .id
                .value()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }
    assert_eq!(first.duration_ms, Some(100));
    let foreign_id = SongId::new(SourceKind::NETEASE, first.id.value());
    assert!(matches!(
        library.songs_detail(&[foreign_id]).await,
        Err(mineral_channel_core::Error::NotFound)
    ));
    let scope = store.scope(SourceKind::LOCAL);
    scope.set_loved(&first.id, true).await?;
    assert!(library.caps().searchable().is_empty());
    assert!(matches!(
        library
            .search_songs("first", mineral_channel_core::Page::default())
            .await,
        Err(mineral_channel_core::Error::NotSupported)
    ));

    // Replacing tags at the same path retains identity and refreshes generic favorite metadata.
    use lofty::{
        config::WriteOptions,
        prelude::{Accessor, TagExt},
        tag::{Tag, TagType},
    };
    let mut tag = Tag::new(TagType::RiffInfo);
    tag.set_title("retagged".to_owned());
    tag.save_to_path(root.join("first.wav"), WriteOptions::default())?;
    // Repeated reads keep this instance's initial catalog until the next daemon instance.
    assert_eq!(playlists(&library).await?, first_read);
    library = open(temp.path(), &store);
    assert_eq!(named(&playlists(&library).await?, "retagged")?.id, first.id);
    assert_eq!(
        scope
            .get_meta(&first.id)
            .await?
            .ok_or_else(|| eyre!("missing stored song"))?
            .name,
        "retagged"
    );

    // Replacing the file object at the same path must keep the path identity.
    let replacement = root.join("replacement.wav");
    audio(&replacement)?;
    tag.save_to_path(&replacement, WriteOptions::default())?;
    std::fs::rename(&replacement, root.join("first.wav"))?;
    library = open(temp.path(), &store);
    assert_eq!(named(&playlists(&library).await?, "retagged")?.id, first.id);

    std::fs::rename(root.join("first.wav"), root.join("moved.wav"))?;
    library = open(temp.path(), &store);
    let moved = named(&playlists(&library).await?, "retagged")?;
    assert_ne!(moved.id, first.id);
    assert!(!scope.is_loved(&moved.id).await?);
    assert!(scope.is_loved(&first.id).await?);
    assert!(matches!(
        library.songs_detail(std::slice::from_ref(&first.id)).await,
        Err(mineral_channel_core::Error::NotFound)
    ));
    std::fs::copy(root.join("moved.wav"), root.join("copy.wav"))?;
    library = open(temp.path(), &store);
    let songs = playlists(&library)
        .await?
        .into_iter()
        .flat_map(|playlist| playlist.entries)
        .filter(|entry| entry.song.name == "retagged")
        .collect::<Vec<_>>();
    assert_eq!(songs.len(), 2);
    assert_eq!(
        songs
            .iter()
            .map(|entry| entry.song.id.clone())
            .collect::<FxHashSet<_>>()
            .len(),
        2
    );
    assert!(scope.is_loved(&first.id).await?);

    tokio::fs::write(root.join("moved.lrc"), "[00:00.10]local lyric").await?;
    assert_eq!(library.lyrics(&moved.id).await?.lines.len(), 1);
    let prepared = library
        .resolve(
            PlaybackRequest::new(moved.id.clone(), BitRate::Hires),
            CancellationToken::new(),
        )
        .await?;
    let direct = prepared
        .direct_media()
        .ok_or_else(|| eyre!("missing direct media"))?;
    assert_eq!(
        direct.locator().local_path(),
        Some(root.join("moved.wav").canonicalize()?.as_path())
    );
    assert_eq!(direct.info().format, Some(mineral_model::AudioFormat::Wav));
    let mut opened = prepared
        .open(OpenOptions::new(CancellationToken::new(), 4096))
        .await?;
    assert!(opened.byte_len().is_some_and(|length| length > 44));
    assert!(opened.take_capture().is_none());
    drop(opened);
    drop(library);
    let restored = open(temp.path(), &store);
    assert_eq!(restored.my_playlists().await?.len(), 2);
    std::fs::remove_file(root.join("moved.wav"))?;
    let restored = open(temp.path(), &store);
    assert!(matches!(
        restored.songs_detail(&[moved.id]).await,
        Err(mineral_channel_core::Error::NotFound)
    ));
    Ok(())
}

// macOS rejects these filename bytes before the library can scan them.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn non_unicode_paths_have_distinct_ids() -> color_eyre::Result<()> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let temp = tempfile::tempdir()?;
    let root = temp.path().join("music");
    for byte in [0xfe, 0xff] {
        let directory = root.join(OsString::from_vec(vec![b'a', byte]));
        std::fs::create_dir_all(&directory)?;
        audio(&directory.join(OsString::from_vec(vec![b's', byte, b'.', b'w', b'a', b'v'])))?;
    }
    let store = ServerStore::open(&temp.path().join("mineral.db")).await?;
    let library = open(temp.path(), &store);
    let playlists = playlists(&library).await?;
    assert_eq!(playlists.len(), 2);
    assert_eq!(
        playlists
            .iter()
            .map(|playlist| &playlist.id)
            .collect::<FxHashSet<_>>()
            .len(),
        2
    );
    assert_eq!(
        playlists
            .iter()
            .flat_map(|playlist| &playlist.entries)
            .map(|entry| &entry.song.id)
            .collect::<FxHashSet<_>>()
            .len(),
        2
    );
    Ok(())
}

/// A failed initial read can load the directory when it becomes available.
#[tokio::test]
async fn unavailable_root_can_be_loaded_later() -> color_eyre::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = ServerStore::open(&temp.path().join("mineral.db")).await?;
    let root = temp.path().join("unmounted");
    let library = LocalLibrary::new(store, temp.path().join("covers"), vec![root.clone()]);
    assert!(library.my_playlists().await.is_err());
    std::fs::create_dir(&root)?;
    audio(&root.join("appeared.wav"))?;
    assert_eq!(library.my_playlists().await?.len(), 1);
    Ok(())
}
