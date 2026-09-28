//! Verifies local lyric selection and decoding through real audio files and SQLite.

use std::path::{Path, PathBuf};

use color_eyre::eyre::eyre;
use lofty::{
    TextEncoding,
    config::WriteOptions,
    id3::v2::{Frame, Id3v2Tag, UnsynchronizedTextFrame},
    prelude::{Accessor, TagExt},
    tag::{ItemKey, Tag, TagType},
};
use mineral_channel_core::{MusicChannel, PlaylistLoad};
use mineral_channel_local::LocalLibrary;
use mineral_model::{LineKind, LyricLine, Playlist, Song, Word};
use mineral_persist::ServerStore;

/// Opens the same public library boundary used by the daemon.
fn open(directory: &Path, store: &ServerStore) -> LocalLibrary {
    LocalLibrary::new(
        store.clone(),
        directory.join("covers"),
        vec![directory.join("music")],
    )
}

/// Creates the directory scanned by each isolated library instance.
fn music_directory(directory: &Path) -> color_eyre::Result<PathBuf> {
    let root = directory.join("music");
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

/// Loads complete playlists so all lyric requests use identities from a real scan.
async fn playlists(library: &LocalLibrary) -> color_eyre::Result<Vec<Playlist>> {
    let mut result = Vec::<Playlist>::new();
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

/// Finds a scanned fixture by its title without assuming playlist order.
fn named(playlists: &[Playlist], name: &str) -> color_eyre::Result<Song> {
    playlists
        .iter()
        .flat_map(|playlist| &playlist.entries)
        .find(|entry| entry.song.name == name)
        .map(|entry| entry.song.clone())
        .ok_or_else(|| eyre!("missing lyric fixture {name}"))
}

/// Writes exactly 100 milliseconds of mono PCM as a taggable WAV container.
fn wav(root: &Path, name: &str) -> color_eyre::Result<PathBuf> {
    let path = root.join(format!("{name}.wav"));
    mineral_test::write_wav(&path, &vec![0; 4_800], 1, 48_000)?;
    Ok(path)
}

/// Copies committed audio before adding tags; tests never invoke an encoder.
fn fixture(root: &Path, source: &str, destination: &str) -> color_eyre::Result<PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(source);
    let path = root.join(destination);
    std::fs::copy(source, &path)?;
    Ok(path)
}

/// Builds a native ID3v2 tag from a title and the given frames.
fn id3(title: &str, frames: Vec<Frame<'static>>) -> Id3v2Tag {
    let mut tag = Id3v2Tag::new();
    tag.set_title(title.to_owned());
    for frame in frames {
        tag.insert(frame);
    }
    tag
}

/// Lets Lofty write the ID3 header and the WAV chunk or MPEG prefix.
fn write_id3(path: &Path, title: &str, frames: Vec<Frame<'static>>) -> color_eyre::Result<()> {
    id3(title, frames).save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// Prepends a Lofty-serialized tag because Lofty only permits reading ID3v2 from FLAC.
fn prepend_id3(path: &Path, title: &str, frames: Vec<Frame<'static>>) -> color_eyre::Result<()> {
    let mut bytes = Vec::<u8>::new();
    id3(title, frames).dump_to(&mut bytes, WriteOptions::default())?;
    bytes.extend(std::fs::read(path)?);
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Writes a real container's native text-lyrics field through ItemKey::Lyrics.
fn write_text_tag(
    path: &Path,
    tag_type: TagType,
    title: &str,
    lyrics: Option<&str>,
) -> color_eyre::Result<()> {
    let mut tag = Tag::new(tag_type);
    tag.set_title(title.to_owned());
    if let Some(lyrics) = lyrics {
        assert!(tag.insert_text(ItemKey::Lyrics, lyrics.to_owned()));
    }
    tag.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// Builds USLT with Lofty's writer, including its language and description fields.
fn uslt(description: &str, text: &str) -> Frame<'static> {
    Frame::UnsynchronizedText(UnsynchronizedTextFrame::new(
        TextEncoding::UTF8,
        *b"eng",
        description.to_owned(),
        text.to_owned(),
    ))
}

/// Encodes sidecar text as UTF-16 with a byte order mark.
fn utf16_with_bom(text: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = Vec::<u8>::new();
    bytes.extend_from_slice(if little_endian {
        &[0xff, 0xfe]
    } else {
        &[0xfe, 0xff]
    });
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

/// Builds an expected word timeline from literal millisecond values, without parsing a payload.
fn word_line(start_ms: u64, dur_ms: u64, words: &[(u64, u64, &str)]) -> LyricLine {
    LyricLine {
        time_ms: Some(start_ms),
        kind: LineKind::Words {
            dur_ms,
            words: words
                .iter()
                .map(|(start_ms, dur_ms, text)| Word {
                    start_ms: *start_ms,
                    dur_ms: *dur_ms,
                    text: (*text).to_owned(),
                })
                .collect::<Vec<_>>(),
        },
        translation: None,
        romanization: None,
    }
}

/// Reads plain text, standard LRC and Enhanced LRC from a real WAV USLT frame.
#[tokio::test]
async fn uslt_plain_lrc_and_enhanced_text() -> color_eyre::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = music_directory(temp.path())?;
    for (name, text) in [
        ("plain", "caf\u{e9}\n\u{6b4c}"),
        ("lrc", "[ti:metadata]\n[00:00.01]first\n[00:00.04]second"),
        (
            "enhanced",
            "[00:00.01]<00:00.01>first <00:00.04>second<00:00.05>",
        ),
    ] {
        write_id3(&wav(&root, name)?, name, vec![uslt("", text)])?;
    }
    let store = ServerStore::open(&temp.path().join("mineral.db")).await?;
    let library = open(temp.path(), &store);
    let playlists = playlists(&library).await?;
    let plain = named(&playlists, "plain")?;
    assert_eq!(
        library.lyrics(&plain.id).await?.lines,
        vec![
            LyricLine::untimed("caf\u{e9}"),
            LyricLine::untimed("\u{6b4c}")
        ]
    );
    let lrc = named(&playlists, "lrc")?;
    assert_eq!(
        library.lyrics(&lrc.id).await?.lines,
        vec![
            LyricLine::timed(10, "first"),
            LyricLine::timed(40, "second")
        ]
    );
    let enhanced = named(&playlists, "enhanced")?;
    assert_eq!(
        library.lyrics(&enhanced.id).await?.lines,
        vec![word_line(10, 40, &[(10, 30, "first "), (40, 10, "second")])]
    );
    Ok(())
}

/// Selects nonempty and better-timed lyrics across primary ID3v2 and secondary APEv2 tags.
#[tokio::test]
async fn lyrics_selection_checks_all_tags() -> color_eyre::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = music_directory(temp.path())?;
    let cases = [
        (
            "secondary-only",
            None,
            "secondary",
            vec![LyricLine::untimed("secondary")],
        ),
        (
            "timed-secondary",
            Some("plain primary"),
            "[00:00.04]secondary",
            vec![LyricLine::timed(40, "secondary")],
        ),
        (
            "timed-primary",
            Some("[00:00.01]primary"),
            "plain secondary",
            vec![LyricLine::timed(10, "primary")],
        ),
        (
            "empty-primary",
            Some("[00:00.01] \n"),
            "secondary",
            vec![LyricLine::untimed("secondary")],
        ),
        (
            "empty-secondary",
            Some("primary"),
            "[00:00.04] \n",
            vec![LyricLine::untimed("primary")],
        ),
        (
            "equal-quality",
            Some("primary"),
            "secondary",
            vec![LyricLine::untimed("primary")],
        ),
    ];
    for (name, primary, secondary, _) in &cases {
        let path = fixture(&root, "silence-mpeg1.mp3", &format!("{name}.mp3"))?;
        let frames = primary
            .iter()
            .map(|text| uslt("", text))
            .collect::<Vec<_>>();
        write_id3(&path, name, frames)?;
        write_text_tag(&path, TagType::Ape, name, Some(secondary))?;
    }
    let store = ServerStore::open(&temp.path().join("mineral.db")).await?;
    let library = open(temp.path(), &store);
    let playlists = playlists(&library).await?;
    assert_eq!(
        playlists
            .iter()
            .map(|playlist| playlist.entries.len())
            .sum::<usize>(),
        cases.len()
    );
    for (name, _, _, expected) in cases {
        let song = named(&playlists, name)?;
        assert_eq!(library.lyrics(&song.id).await?.lines, expected, "{name}");
    }
    Ok(())
}

/// Reads native Vorbis, MP4 and APEv2 lyric fields without converting them to ID3v2 first.
#[tokio::test]
async fn native_container_lyrics_use_item_key_lyrics() -> color_eyre::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = music_directory(temp.path())?;
    let cases = [
        (
            "silence.flac",
            "flac",
            TagType::VorbisComments,
            "caf\u{e9}\n\u{6b4c}",
            vec![
                LyricLine::untimed("caf\u{e9}"),
                LyricLine::untimed("\u{6b4c}"),
            ],
        ),
        (
            "silence.ogg",
            "ogg",
            TagType::VorbisComments,
            "[00:00.01]Vorbis\n[00:00.04]\u{6b4c}",
            vec![
                LyricLine::timed(10, "Vorbis"),
                LyricLine::timed(40, "\u{6b4c}"),
            ],
        ),
        (
            "silence.m4a",
            "mp4",
            TagType::Mp4Ilst,
            "[00:00.02]MP4\n[00:00.05]\u{1f3b5}",
            vec![
                LyricLine::timed(20, "MP4"),
                LyricLine::timed(50, "\u{1f3b5}"),
            ],
        ),
        (
            "silence-mpeg1.mp3",
            "ape",
            TagType::Ape,
            "APE lyrics\n\u{6b4c}",
            vec![
                LyricLine::untimed("APE lyrics"),
                LyricLine::untimed("\u{6b4c}"),
            ],
        ),
    ];
    for (source, name, tag_type, text, _) in &cases {
        let path = fixture(&root, source, source)?;
        write_text_tag(&path, *tag_type, name, Some(text))?;
    }
    let store = ServerStore::open(&temp.path().join("mineral.db")).await?;
    let library = open(temp.path(), &store);
    let playlists = playlists(&library).await?;
    for (_, name, _, _, expected) in cases {
        let song = named(&playlists, name)?;
        assert_eq!(library.lyrics(&song.id).await?.lines, expected, "{name}");
    }
    Ok(())
}

/// Reads secondary FLAC ID3v2 lyrics when Vorbis lacks them or only provides line timing.
#[tokio::test]
async fn flac_secondary_id3_is_not_hidden_by_primary_vorbis() -> color_eyre::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = music_directory(temp.path())?;
    let text_path = fixture(&root, "silence.flac", "secondary-text.flac")?;
    write_text_tag(&text_path, TagType::VorbisComments, "secondary-text", None)?;
    prepend_id3(
        &text_path,
        "ignored-secondary-title",
        vec![uslt("", "secondary lyrics")],
    )?;
    let words_path = fixture(&root, "silence.flac", "secondary-words.flac")?;
    write_text_tag(
        &words_path,
        TagType::VorbisComments,
        "secondary-words",
        Some("[00:00.01]line alternative"),
    )?;
    prepend_id3(
        &words_path,
        "ignored-secondary-title",
        vec![uslt(
            "",
            "[00:00.01]<00:00.01>word <00:00.025>timing<00:00.04>\n[00:00.04]last",
        )],
    )?;
    let store = ServerStore::open(&temp.path().join("mineral.db")).await?;
    let library = open(temp.path(), &store);
    let playlists = playlists(&library).await?;
    let text_song = named(&playlists, "secondary-text")?;
    assert_eq!(
        library.lyrics(&text_song.id).await?.lines,
        vec![LyricLine::untimed("secondary lyrics")]
    );
    let words_song = named(&playlists, "secondary-words")?;
    assert_eq!(
        library.lyrics(&words_song.id).await?.lines,
        vec![
            word_line(10, 30, &[(10, 15, "word "), (25, 15, "timing")]),
            LyricLine::timed(40, "last")
        ]
    );
    Ok(())
}

/// Gives freshly read UTF-8 and BOM-marked UTF-16 sidecars priority over embedded word timing.
#[tokio::test]
async fn sidecar_encodings_take_priority_and_are_read_on_each_request() -> color_eyre::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = music_directory(temp.path())?;
    let path = wav(&root, "sidecar")?;
    write_id3(
        &path,
        "sidecar",
        vec![uslt(
            "",
            "[00:00.01]<00:00.01>embed <00:00.03>ded<00:00.10>",
        )],
    )?;
    let store = ServerStore::open(&temp.path().join("mineral.db")).await?;
    let library = open(temp.path(), &store);
    let song = named(&playlists(&library).await?, "sidecar")?;
    let embedded = vec![word_line(10, 90, &[(10, 20, "embed "), (30, 70, "ded")])];
    assert_eq!(library.lyrics(&song.id).await?.lines, embedded);
    let sidecar = path.with_extension("lrc");
    tokio::fs::write(&sidecar, "[00:00.02]UTF-8 \u{6b4c}").await?;
    assert_eq!(
        library.lyrics(&song.id).await?.lines,
        vec![LyricLine::timed(20, "UTF-8 \u{6b4c}")]
    );
    let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
    utf8_bom.extend_from_slice("[00:00.03]UTF-8 BOM \u{1f3b5}".as_bytes());
    tokio::fs::write(&sidecar, utf8_bom).await?;
    assert_eq!(
        library.lyrics(&song.id).await?.lines,
        vec![LyricLine::timed(30, "UTF-8 BOM \u{1f3b5}")]
    );
    for (little_endian, text, time_ms, expected) in [
        (
            true,
            "[00:00.04]UTF-16LE \u{97f3}\u{1f3b5}",
            40,
            "UTF-16LE \u{97f3}\u{1f3b5}",
        ),
        (
            false,
            "[00:00.05]UTF-16BE \u{97f3}\u{1f3b5}",
            50,
            "UTF-16BE \u{97f3}\u{1f3b5}",
        ),
    ] {
        tokio::fs::write(&sidecar, utf16_with_bom(text, little_endian)).await?;
        assert_eq!(
            library.lyrics(&song.id).await?.lines,
            vec![LyricLine::timed(time_ms, expected)]
        );
    }
    tokio::fs::write(&sidecar, "untimed sidecar").await?;
    assert_eq!(
        library.lyrics(&song.id).await?.lines,
        vec![LyricLine::untimed("untimed sidecar")]
    );
    tokio::fs::remove_file(&sidecar).await?;
    assert_eq!(library.lyrics(&song.id).await?.lines, embedded);
    assert_eq!(named(&playlists(&library).await?, "sidecar")?.id, song.id);
    Ok(())
}
