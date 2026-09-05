//! `mineral status` — connect daemon socket,拉一次 audio snapshot 打印。
//!
//! 验证 IPC 链路是否通。daemon 没起 / socket 文件 stale / 版本不匹配 → 友好报错
//! (握手与配对语义在 [`OneshotClient`] 内)。

use color_eyre::eyre::bail;
use mineral_audio::{AudioBackend, AudioSnapshot};
use mineral_protocol::{DownloadSummary, OneshotClient, Request, Response};

/// `mineral status` 入口:连 daemon socket(含握手)→ 依次拉快照 / pid / 下载进度 → 打印。
pub async fn run() -> color_eyre::Result<()> {
    let socket_path = mineral_paths::socket_path()?;
    let mut client = OneshotClient::connect(&socket_path).await?;

    let snap = match client.request(Request::AudioSnapshot).await? {
        Response::AudioSnapshot(snap) => snap,
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    };

    let pid = match client.request(Request::DaemonInfo).await? {
        Response::DaemonInfo { pid } => pid,
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    };

    let summary = match client.request(Request::DownloadSummary).await? {
        Response::DownloadSummary(summary) => summary,
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    };

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

#[cfg(test)]
mod tests {
    use super::render_snapshot;
    use mineral_audio::{AudioBackend, AudioSnapshot};

    /// 正常后端:backend 行打 `device`,且首行打出 daemon pid。
    #[test]
    fn render_device_backend() {
        let snap = AudioSnapshot {
            backend: AudioBackend::Device,
            ..AudioSnapshot::default()
        };
        let out = render_snapshot(&snap, /*pid*/ 4242);
        assert!(out.contains("backend:    device"), "实际:\n{out}");
        assert!(
            out.contains("pid:        4242"),
            "应打出 daemon pid:\n{out}"
        );
        assert!(
            !out.contains("no audio device"),
            "device 态不该提示无设备:\n{out}"
        );
    }

    /// 降级后端:backend 行提示 `null (no audio device)`。
    #[test]
    fn render_null_backend() {
        let snap = AudioSnapshot {
            backend: AudioBackend::Null,
            ..AudioSnapshot::default()
        };
        let out = render_snapshot(&snap, /*pid*/ 4242);
        assert!(
            out.contains("backend:    null (no audio device)"),
            "实际:\n{out}"
        );
    }
}
