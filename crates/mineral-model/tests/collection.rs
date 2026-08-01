//! Collection membership model 的 public contract 回归测试。

use mineral_model::{
    Album, AlbumId, AlbumTrack, CollectionIndex, Playlist, PlaylistEntry, PlaylistId, Song, SongId,
    SourceKind,
};

/// 构造 relation serde contract 使用的 Song。
fn song(value: &str) -> Song {
    Song::builder()
        .id(SongId::new(SourceKind::NETEASE, value))
        .name(value.to_owned())
        .build()
}

/// CollectionIndex 使用 fixed-width 0-based 数字透明序列化。
#[test]
fn collection_index_serde_is_transparent_u64() -> color_eyre::Result<()> {
    let index = CollectionIndex::new(7);
    let json = serde_json::to_value(index)?;
    assert_eq!(json, serde_json::json!(7));
    assert_eq!(serde_json::from_value::<CollectionIndex>(json)?.get(), 7);
    Ok(())
}

/// AlbumTrack round-trip 保留 index 与 Song，而 Album 只保存 relation list。
#[test]
fn album_track_round_trip_preserves_relation() -> color_eyre::Result<()> {
    let album = Album::builder()
        .id(AlbumId::new(SourceKind::NETEASE, "album"))
        .name("album".to_owned())
        .tracks(vec![
            AlbumTrack::builder()
                .index(CollectionIndex::new(4))
                .song(song("song"))
                .build(),
        ])
        .build();
    let round_trip = serde_json::from_value::<Album>(serde_json::to_value(&album)?)?;
    assert_eq!(round_trip, album);
    assert_eq!(
        round_trip.tracks.first().map(|track| track.index.get()),
        Some(4)
    );
    Ok(())
}

/// PlaylistEntry round-trip 保留 non-contiguous index，不由 Vec coordinate 重算。
#[test]
fn playlist_entry_round_trip_preserves_index_gap() -> color_eyre::Result<()> {
    let playlist = Playlist::builder()
        .id(PlaylistId::new(SourceKind::NETEASE, "playlist"))
        .name("playlist".to_owned())
        .entries(vec![
            PlaylistEntry::builder()
                .index(CollectionIndex::new(1))
                .song(song("first-loaded"))
                .build(),
            PlaylistEntry::builder()
                .index(CollectionIndex::new(9))
                .song(song("second-loaded"))
                .build(),
        ])
        .build();
    let round_trip = serde_json::from_value::<Playlist>(serde_json::to_value(&playlist)?)?;
    assert_eq!(round_trip, playlist);
    assert_eq!(
        round_trip
            .entries
            .iter()
            .map(|entry| entry.index.get())
            .collect::<Vec<u64>>(),
        vec![1, 9]
    );
    Ok(())
}
