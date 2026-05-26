//! `mineral status` — connect daemon socket,拉一次 audio snapshot 打印。
//!
//! 验证 IPC 链路是否通。daemon 没起 / socket 文件 stale → 友好报错。

use color_eyre::eyre::{WrapErr, bail, eyre};
use mineral_audio::{AudioBackend, AudioSnapshot};
use mineral_protocol::{DownloadProgress, Request, Response, framed, recv, send};
use tokio::net::UnixStream;

/// `mineral status` 入口:连 daemon socket → 发 [`Request::AudioSnapshot`] → 打印结果。
pub async fn run() -> color_eyre::Result<()> {
    let socket_path = mineral_paths::runtime_dir()
        .wrap_err("resolve runtime_dir")?
        .join("mineral.sock");
    let stream = UnixStream::connect(&socket_path).await.wrap_err_with(|| {
        format!(
            "connect daemon socket {} (run `mineral serve` first?)",
            socket_path.display()
        )
    })?;
    let mut conn = framed(stream);

    send(&mut conn, &Request::AudioSnapshot).await?;
    let snap = match recv::<Response, _>(&mut conn)
        .await?
        .ok_or_else(|| eyre!("daemon closed connection unexpectedly"))?
    {
        Response::AudioSnapshot(snap) => snap,
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    };

    send(&mut conn, &Request::DaemonInfo).await?;
    let pid = match recv::<Response, _>(&mut conn)
        .await?
        .ok_or_else(|| eyre!("daemon closed connection unexpectedly"))?
    {
        Response::DaemonInfo { pid } => pid,
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    };

    send(&mut conn, &Request::DownloadProgress).await?;
    let progress = match recv::<Response, _>(&mut conn)
        .await?
        .ok_or_else(|| eyre!("daemon closed connection unexpectedly"))?
    {
        Response::DownloadProgress(p) => p,
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    };

    let download = if progress.active {
        format!("\ndownload:   {}", render_download(&progress))
    } else {
        String::new()
    };
    println!("{}{download}", render_snapshot(&snap, pid));
    Ok(())
}

/// 把下载进度渲染成一行:`3/12  62%  2.4 MB/s`(仅 `active` 时由 caller 打出)。
fn render_download(dp: &DownloadProgress) -> String {
    let pct = dp
        .bytes_done
        .saturating_mul(100)
        .checked_div(dp.bytes_total)
        .unwrap_or(0)
        .min(100);
    let cur = dp.done.saturating_add(1).min(dp.total.max(1));
    format!("{cur}/{}  {pct}%  {}", dp.total, format_speed(dp.speed_bps))
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
    let dur = format_ms(snap.duration_ms);
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
