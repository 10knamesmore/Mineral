//! Wire-friendly 化的回归测试:验证 ClientHandle 暴露面上所有 wire-bound 类型
//! 都能 serde round-trip。这是 IPC 化的硬前置——这个测试一直绿,IPC transport
//! 落地时只需在 mineral-server 内加一层编解码 adapter,客户端调用方零改动。
//!
//! `ClientHandle` 本身不在测试范围:它内部持 Arc handle 是同进程实现细节,
//! 跨进程时整体被 RemoteClient 替代。这里只测「方法签名上出现的 enum / id /
//! snapshot / filter」是否真能过 wire。

use mineral_audio::AudioSnapshot;
use mineral_model::{PlaylistId, SongId, SourceKind};
use mineral_server::{CancelFilter, ChannelFetchKindTag};
use mineral_task::{ChannelFetchKind, Priority, TaskId, TaskKind};

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
        duration_ms: 200_000,
        volume_pct: 77,
        track_finished_seq: 3,
    };
    let back = round_trip(&snap)?;
    assert_eq!(snap, back);
    Ok(())
}

#[test]
fn task_kind_round_trip() -> color_eyre::Result<()> {
    let cases = vec![
        TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists {
            source: SourceKind::Netease,
        }),
        TaskKind::ChannelFetch(ChannelFetchKind::PlaylistTracks {
            source: SourceKind::Netease,
            id: PlaylistId::new("p123".to_owned()),
        }),
        TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
            source: SourceKind::Netease,
            song_id: SongId::new("s456".to_owned()),
        }),
        TaskKind::ChannelFetch(ChannelFetchKind::Lyrics {
            source: SourceKind::Netease,
            song_id: SongId::new("s456".to_owned()),
        }),
        TaskKind::CoverArt {
            url: mineral_model::MediaUrl::remote("https://example.com/c.jpg")?,
        },
    ];
    for k in &cases {
        let back = round_trip(k)?;
        assert_eq!(k, &back);
    }
    Ok(())
}

#[test]
fn priority_and_task_id_round_trip() -> color_eyre::Result<()> {
    for p in [Priority::Background, Priority::User] {
        let back = round_trip(&p)?;
        assert_eq!(p, back);
    }
    // TaskId 内部 u64,我们没有公开构造器,但 Default-able 类型可以塞个数字 JSON 反过来 parse
    let id_json = "42";
    let id: TaskId = serde_json::from_str(id_json)?;
    let s = serde_json::to_string(&id)?;
    assert_eq!(s, id_json);
    Ok(())
}

#[test]
fn cancel_filter_round_trip() -> color_eyre::Result<()> {
    let cases = vec![
        CancelFilter::ChannelFetchKinds(vec![
            ChannelFetchKindTag::SongUrl,
            ChannelFetchKindTag::Lyrics,
        ]),
        CancelFilter::ChannelFetchKinds(vec![]),
        CancelFilter::CoverArt,
    ];
    for f in &cases {
        let back = round_trip(f)?;
        assert_eq!(f, &back);
    }
    Ok(())
}

#[test]
fn cancel_filter_matches_only_intended_kinds() -> color_eyre::Result<()> {
    let songurl = TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
        source: SourceKind::Netease,
        song_id: SongId::new("s".to_owned()),
    });
    let myplaylists = TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists {
        source: SourceKind::Netease,
    });
    let cover = TaskKind::CoverArt {
        url: mineral_model::MediaUrl::remote("https://example.com/c.jpg")?,
    };

    let f = CancelFilter::ChannelFetchKinds(vec![ChannelFetchKindTag::SongUrl]);
    assert!(f.matches(&songurl));
    assert!(!f.matches(&myplaylists));
    assert!(!f.matches(&cover));

    let f = CancelFilter::CoverArt;
    assert!(!f.matches(&songurl));
    assert!(f.matches(&cover));
    Ok(())
}
