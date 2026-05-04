//! `mineral status` — connect daemon socket,拉一次 audio snapshot 打印。
//!
//! 验证 IPC 链路是否通。daemon 没起 / socket 文件 stale → 友好报错。

use color_eyre::eyre::{WrapErr, bail, eyre};
use mineral_audio::AudioSnapshot;
use mineral_protocol::{Request, Response, framed, recv, send};
use tokio::net::UnixStream;

pub async fn run() -> color_eyre::Result<()> {
    let socket_path = mineral_paths::runtime_dir()
        .wrap_err("resolve runtime_dir")?
        .join("mineral.sock");
    let stream = UnixStream::connect(&socket_path)
        .await
        .wrap_err_with(|| {
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
        Response::AudioSnapshot(snap) => print_snapshot(&snap),
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    }
    Ok(())
}

fn print_snapshot(snap: &AudioSnapshot) {
    let pos = format_ms(snap.position_ms);
    let dur = format_ms(snap.duration_ms);
    println!("playing:    {}", snap.playing);
    println!("position:   {pos} / {dur}");
    println!("volume:     {} %", snap.volume_pct);
    println!("finished:   {} (track_finished_seq)", snap.track_finished_seq);
}

fn format_ms(ms: u64) -> String {
    let s = ms / 1000;
    let m = s / 60;
    let s = s % 60;
    format!("{m:02}:{s:02}")
}
