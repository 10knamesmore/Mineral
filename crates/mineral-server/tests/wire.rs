//! 验证 client/server 边界上的任务与事件类型能经过 serde 往返。

use mineral_audio::AudioSnapshot;
use mineral_model::{PlaylistId, SongId, SourceKind};
use mineral_task::{ChannelFetchKind, Priority, TaskEvent, TaskKind};
use rustc_hash::FxHashSet;

fn round_trip<T>(v: &T) -> color_eyre::Result<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let s = serde_json::to_string(v)?;
    Ok(serde_json::from_str(&s)?)
}

#[test]
fn audio_snapshot_round_trip() -> color_eyre::Result<()> {
    let snap = AudioSnapshot {
        playing: true,
        position_ms: 12_345,
        duration_ms: Some(200_000),
        volume_pct: 77,
        track_finished_seq: 3,
        backend: mineral_audio::AudioBackend::Null,
        buffered_bps: mineral_audio::Bps::new(6_000),
        current_track_token: 9,
        next_duration_ms: Some(180_000),
        next_buffered_bps: mineral_audio::Bps::new(6_000),
        next_ready: true,
        sample_rate_hz: 44_100,
    };
    let back = round_trip(&snap)?;
    assert_eq!(snap, back);
    Ok(())
}

#[test]
fn task_kind_round_trip() -> color_eyre::Result<()> {
    let cases = vec![
        TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists {
            source: SourceKind::NETEASE,
        }),
        TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail {
            id: PlaylistId::new(SourceKind::NETEASE, "p123"),
        }),
        TaskKind::ChannelFetch(ChannelFetchKind::Lyrics {
            song_id: SongId::new(SourceKind::NETEASE, "s456"),
        }),
    ];
    for k in &cases {
        let back = round_trip(k)?;
        assert_eq!(k, &back);
    }
    Ok(())
}

#[test]
fn priority_round_trip() -> color_eyre::Result<()> {
    for p in [Priority::Background, Priority::User] {
        let back = round_trip(&p)?;
        assert_eq!(p, back);
    }
    Ok(())
}

#[test]
fn task_event_round_trip() -> color_eyre::Result<()> {
    // TaskEvent 没派 PartialEq(Lyrics/Playlist 等也没),所以只验证能编译并 decode。
    let cases = vec![
        TaskEvent::PlaylistsFetched {
            source: SourceKind::NETEASE,
            playlists: vec![],
        },
        TaskEvent::LikedSongIdsFetched {
            source: SourceKind::NETEASE,
            ids: FxHashSet::default(),
        },
        TaskEvent::LocalPlayCountFetched {
            song_id: SongId::new(SourceKind::NETEASE, "s"),
            count: Some(7),
        },
        TaskEvent::PlaylistDetailFetched {
            id: PlaylistId::new(SourceKind::NETEASE, "p"),
            playlist: Box::new(
                mineral_model::Playlist::builder()
                    .id(PlaylistId::new(SourceKind::NETEASE, "p"))
                    .name(String::new())
                    .build(),
            ),
        },
    ];
    for ev in &cases {
        let s = serde_json::to_string(ev)?;
        let _back: TaskEvent = serde_json::from_str(&s)?;
    }
    Ok(())
}
