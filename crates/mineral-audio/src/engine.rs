//! 引擎线程主体:owns `OutputStream` / `Sink` / 内嵌 tokio runtime,顺序处理命令并刷 snapshot。

use std::io::BufReader;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::eyre;
use mineral_model::MediaUrl;
use parking_lot::Mutex;
use rodio::Source;
use stream_download::http::reqwest::Client;
use stream_download::http::HttpStream;
use stream_download::source::SourceStream;
use stream_download::storage::temp::TempStorageProvider;
use stream_download::Settings;
use stream_download::StreamDownload;

use crate::command::AudioCommand;
use crate::snapshot::AudioSnapshot;

/// 命令通道空转间隔 / snapshot 刷新节拍。
const TICK: Duration = Duration::from_millis(100);

/// 默认初始音量,与 UI 默认 66% 对齐。
const DEFAULT_VOLUME_F32: f32 = 0.66;

/// 引擎线程入口。
///
/// `ready_tx` 在引擎完成 sink/runtime 初始化后立刻汇报,UI 才返回 handle。
pub(crate) fn run(
    cmd_rx: &mpsc::Receiver<AudioCommand>,
    snapshot: &Arc<Mutex<AudioSnapshot>>,
    ready_tx: &mpsc::SyncSender<color_eyre::Result<()>>,
) {
    if let Err(e) = engine_main(cmd_rx, snapshot, ready_tx) {
        mineral_log::warn!(target: "audio_engine", "exited: {e:?}");
    }
}

fn engine_main(
    cmd_rx: &mpsc::Receiver<AudioCommand>,
    snapshot: &Arc<Mutex<AudioSnapshot>>,
    ready_tx: &mpsc::SyncSender<color_eyre::Result<()>>,
) -> color_eyre::Result<()> {
    let (_stream, stream_handle) = match rodio::OutputStream::try_default() {
        Ok(v) => v,
        Err(e) => {
            let err = eyre!("rodio output stream: {e}");
            let _ = ready_tx.send(Err(eyre!("rodio output stream: {e}")));
            return Err(err);
        }
    };
    let sink = match rodio::Sink::try_new(&stream_handle) {
        Ok(s) => s,
        Err(e) => {
            let err = eyre!("rodio sink: {e}");
            let _ = ready_tx.send(Err(eyre!("rodio sink: {e}")));
            return Err(err);
        }
    };
    sink.set_volume(DEFAULT_VOLUME_F32);

    // multi_thread:stream-download 的下载后台 task 必须在独立 worker 上持续被 poll,
    // 否则 block_on 一返回,reader.read 永远等不到字节,sink 一直空 → UI 一直 paused。
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("mineral-audio-rt")
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            let err = eyre!("tokio runtime: {e}");
            let _ = ready_tx.send(Err(eyre!("tokio runtime: {e}")));
            return Err(err);
        }
    };

    let _ = ready_tx.send(Ok(()));

    let mut cur_duration_ms: u64 = 0;

    loop {
        match cmd_rx.recv_timeout(TICK) {
            Ok(cmd) => match handle_command(cmd, &sink, &rt) {
                Ok(new_dur) => {
                    if let Some(d) = new_dur {
                        cur_duration_ms = d;
                    }
                }
                Err(e) => {
                    mineral_log::warn!(target: "audio_engine", "command error: {e:?}");
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        update_snapshot(snapshot, &sink, cur_duration_ms);
    }
    Ok(())
}

/// 处理一条命令。返回 `Some(duration_ms)` 表示曲目变了,sink 现在播这个新曲;
/// `None` 表示 duration 没变(暂停/恢复/音量等)。
fn handle_command(
    cmd: AudioCommand,
    sink: &rodio::Sink,
    rt: &tokio::runtime::Runtime,
) -> color_eyre::Result<Option<u64>> {
    match cmd {
        AudioCommand::Play(url) => {
            sink.stop();
            let dur = append_decoded(sink, rt, url)?;
            sink.play();
            Ok(Some(dur))
        }
        AudioCommand::Pause => {
            sink.pause();
            Ok(None)
        }
        AudioCommand::Resume => {
            sink.play();
            Ok(None)
        }
        AudioCommand::Stop => {
            sink.stop();
            Ok(Some(0))
        }
        AudioCommand::Seek(ms) => {
            sink.try_seek(Duration::from_millis(ms))
                .map_err(|e| eyre!("seek: {e}"))?;
            Ok(None)
        }
        AudioCommand::SetVolume(pct) => {
            sink.set_volume(f32::from(pct) / 100.0);
            Ok(None)
        }
    }
}

/// 把 URL 解析成 decoder 并 append 到 sink。返回 decoder 探到的 duration(ms,0 = 未知)。
fn append_decoded(
    sink: &rodio::Sink,
    rt: &tokio::runtime::Runtime,
    url: MediaUrl,
) -> color_eyre::Result<u64> {
    match url {
        MediaUrl::Remote(u) => {
            let reader = rt.block_on(async {
                let stream = HttpStream::<Client>::create(u)
                    .await
                    .map_err(|e| eyre!("http stream: {e}"))?;
                StreamDownload::from_stream(stream, TempStorageProvider::new(), Settings::default())
                    .await
                    .map_err(|e| eyre!("stream-download init: {e}"))
            })?;
            let decoder = rodio::Decoder::new(reader).map_err(|e| eyre!("decode: {e}"))?;
            let dur_ms = decoder.total_duration().map(duration_to_ms).unwrap_or(0);
            sink.append(decoder);
            Ok(dur_ms)
        }
        MediaUrl::Local(p) => {
            let file = std::fs::File::open(&p).map_err(|e| eyre!("open {}: {e}", p.display()))?;
            let reader = BufReader::new(file);
            let decoder = rodio::Decoder::new(reader).map_err(|e| eyre!("decode: {e}"))?;
            let dur_ms = decoder.total_duration().map(duration_to_ms).unwrap_or(0);
            sink.append(decoder);
            Ok(dur_ms)
        }
    }
}

fn update_snapshot(snapshot: &Arc<Mutex<AudioSnapshot>>, sink: &rodio::Sink, cur_duration_ms: u64) {
    let pos_ms = duration_to_ms(sink.get_pos());
    let playing = !sink.is_paused() && !sink.empty();
    let mut g = snapshot.lock();
    g.playing = playing;
    g.position_ms = pos_ms;
    g.duration_ms = cur_duration_ms;
    // volume_pct 由 handle.set_volume 直接维护,引擎不反查。
}

fn duration_to_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
