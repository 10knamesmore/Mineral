//! `mineral status` — 连 daemon socket,订阅播放锚点与下载摘要,打印一次快照。
//!
//! 验证会话链路是否通。daemon 没起 / socket 文件 stale / 版本不匹配 → 友好报错
//! (握手与配对语义在 [`mineral_client::Client`] 内)。

use std::time::Duration;

use color_eyre::eyre::{WrapErr, bail};
use mineral_audio::{AudioBackend, AudioSnapshot};
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
use mineral_protocol::{DownloadSummary, SocketWire, SubscriptionTopic};

/// 等待 daemon 推送首帧订阅数据的上限(daemon 订阅即推,正常在毫秒级)。
const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// `mineral status` 入口:连 daemon(含握手)→ 订阅播放 / 下载 → 打印一次快照。
pub async fn run() -> color_eyre::Result<()> {
    let socket_path = mineral_paths::socket_path()?;
    let wire = SocketWire::connect(&socket_path)
        .await
        .wrap_err("连不上 daemon;先跑 `mineral serve`")?;
    let client = Client::from_wire(Box::new(wire), "mineral_status", ClientConfig::default())
        .await
        .wrap_err("连不上 daemon;先跑 `mineral serve`")?;
    client.subscribe(SubscriptionTopic::Playback);
    client.subscribe(SubscriptionTopic::DownloadsSummary);
    client.wait_subscriptions_ready(READY_TIMEOUT).await;

    let pid = match client.daemon_info().await {
        Outcome::Applied(pid) => pid,
        Outcome::Accepted(pid) => pid,
        Outcome::Failed { detail, .. } | Outcome::Unknown { detail } => {
            bail!("daemon error: {detail}")
        }
    };

    let mut snap = client.playback_snapshot();
    snap.position_ms = client.playback_position_ms();
    let summary = client
        .mirror()
        .read_downloads_summary(DownloadSummary::clone);

    let download = if summary.active > 0 || summary.queued > 0 || summary.preparing_playlists > 0 {
        format!("\ndownload:   {}", render_download(&summary))
    } else {
        String::new()
    };
    println!("{}{download}", render_snapshot(&snap, pid));
    Ok(())
}

/// Renders active, queued, playlist preparation, and aggregate speed in one line.
fn render_download(summary: &DownloadSummary) -> String {
    format!(
        "{} active  {} queued  {} preparing  {}",
        summary.active,
        summary.queued,
        summary.preparing_playlists,
        format_speed(summary.speed_bps)
    )
}

/// 速度(字节/秒)→ 人读字符串,整数定点。
fn format_speed(bps: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bps >= MB {
        let tenths = bps.saturating_mul(10) / MB;
        format!("{}.{} MB/s", tenths / 10, tenths % 10)
    } else if bps >= KB {
        format!("{} KB/s", bps / KB)
    } else {
        format!("{bps} B/s")
    }
}

/// 把 [`AudioSnapshot`] + daemon pid 渲染成多行 key/value 文本(由 caller 打到 stdout)。
fn render_snapshot(snap: &AudioSnapshot, pid: u32) -> String {
    let pos = format_ms(snap.position_ms);
    // 时长未知(decoder 探不出)画 --:-- 占位,与真实 00:00 区分。
    let dur = snap
        .duration_ms
        .map_or_else(|| "--:--".to_owned(), format_ms);
    let backend = match snap.backend {
        AudioBackend::Device => "device",
        AudioBackend::Null => "null (no audio device)",
    };
    format!(
        "pid:        {pid}\nplaying:    {}\nposition:   {pos} / {dur}\nvolume:     {} %\nfinished:   {} (track_finished_seq)\nbackend:    {backend}",
        snap.playing, snap.volume_pct, snap.track_finished_seq,
    )
}

/// 把 ms 格式化成 `mm:ss`(小时被合并进分钟)。
fn format_ms(ms: u64) -> String {
    let s = ms / 1000;
    let m = s / 60;
    let s = s % 60;
    format!("{m:02}:{s:02}")
}
