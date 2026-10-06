//! 投递入口的未挂接与无响应线程边界;不依赖 daemon 配置加载。

use std::time::Duration;

use mineral_model::BitRate;
use mineral_test::song;

use super::ScriptSender;
use crate::{BeforeStreamCtx, CurateOutcome, HookDecision, HookMode};

/// 消息无接收线程或线程未回应时,音乐拦截与策展都按既有策略放行。
#[tokio::test]
async fn detached_and_unresponsive_sender_fail_open() -> color_eyre::Result<()> {
    let detached = ScriptSender::detached();
    assert_eq!(
        detached
            .intercept_stream(stream_ctx()?, Duration::from_secs(60))
            .await,
        HookDecision::Continue
    );
    let (tx, rx) = std::sync::mpsc::channel();
    detached.attach(tx);
    let started = std::time::Instant::now();
    assert_eq!(
        detached
            .intercept_stream(stream_ctx()?, Duration::from_millis(50))
            .await,
        HookDecision::Continue
    );
    assert!(started.elapsed() >= Duration::from_millis(50));
    assert_eq!(
        detached
            .curate_playlists(None, Vec::new(), Duration::from_millis(50))
            .await,
        CurateOutcome::Identity
    );
    drop(rx);
    Ok(())
}

/// 有原始直链的 before_stream 快照;投递测试不求值 Lua 配置。
fn stream_ctx() -> color_eyre::Result<BeforeStreamCtx> {
    let target = song("1");
    let direct = mineral_model::DirectMedia::remote(
        mineral_model::PlaybackMediaInfo {
            song_id: target.id.clone(),
            bitrate_bps: None,
            size: None,
            format: Some(mineral_model::AudioFormat::Flac),
            bit_depth: None,
            substituted: false,
        },
        "https://example.com/a.flac".parse()?,
        Vec::new(),
        mineral_model::StreamLayout::Contiguous,
    );
    Ok(BeforeStreamCtx::playable(
        target,
        BitRate::Exhigh,
        HookMode::Immediate,
        Some(direct),
    ))
}
