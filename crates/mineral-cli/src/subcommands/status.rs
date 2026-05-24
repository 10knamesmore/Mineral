//! `mineral status` — connect daemon socket,拉一次 audio snapshot 打印。
//!
//! 验证 IPC 链路是否通。daemon 没起 / socket 文件 stale → 友好报错。

use color_eyre::eyre::{WrapErr, bail, eyre};
use mineral_audio::{AudioBackend, AudioSnapshot};
use mineral_protocol::{Request, Response, framed, recv, send};
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
    let resp = recv::<Response, _>(&mut conn)
        .await?
        .ok_or_else(|| eyre!("daemon closed connection unexpectedly"))?;
    match resp {
        Response::AudioSnapshot(snap) => println!("{}", render_snapshot(&snap)),
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    }
    Ok(())
}

/// 把 [`AudioSnapshot`] 渲染成多行 key/value 文本(由 caller 打到 stdout)。
fn render_snapshot(snap: &AudioSnapshot) -> String {
    let pos = format_ms(snap.position_ms);
    let dur = format_ms(snap.duration_ms);
    let backend = match snap.backend {
        AudioBackend::Device => "device",
        AudioBackend::Null => "null (no audio device)",
    };
    format!(
        "playing:    {}\nposition:   {pos} / {dur}\nvolume:     {} %\nfinished:   {} (track_finished_seq)\nbackend:    {backend}",
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

    /// 正常后端:backend 行打 `device`。
    #[test]
    fn render_device_backend() {
        let snap = AudioSnapshot {
            backend: AudioBackend::Device,
            ..AudioSnapshot::default()
        };
        let out = render_snapshot(&snap);
        assert!(out.contains("backend:    device"), "实际:\n{out}");
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
        let out = render_snapshot(&snap);
        assert!(
            out.contains("backend:    null (no audio device)"),
            "实际:\n{out}"
        );
    }
}
