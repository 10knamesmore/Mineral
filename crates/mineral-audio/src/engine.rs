//! 引擎线程主体:owns rodio device sink + Player + 内嵌 tokio runtime。
//!
//! 命令通道处理 play/pause/resume/stop/set_volume(语义不可合并)。
//! seek 单独走 [`crate::handle::AudioHandle`] → mailbox(latest-wins),engine 每个 tick
//! `take()` 一次实际打 demuxer ——抗住长按 ←/→ 的 30Hz key-repeat。

use std::io::BufReader;
use std::sync::atomic::AtomicU32;
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
use crate::tap::{SharedProd, TapSource};

/// 命令通道空转间隔 / snapshot 刷新节拍 / seek mailbox drain 节拍。
///
/// 20ms 是经验值:OS 键盘 key-repeat 一般 ~30Hz(33ms 一次),20ms tick 能在
/// 用户长按 ←/→ 时把 mailbox 几乎实时 drain → seek,感觉上接近连续(否则两次
/// seek 之间的 tick 间隙会播放旧位置的几十 ms 音频,听感就是「跳一下播一下」)。
/// 同样 stop 命令延迟 ≤20ms,切歌时旧曲尾巴可被压到 cpal 回调缓冲固有长度。
/// termusic 用 5ms,我们留余量。
const TICK: Duration = Duration::from_millis(20);

/// 默认初始音量百分比,与 UI 默认 66% 对齐。换算成 cubic gain ≈ 0.287。
const DEFAULT_VOLUME_PCT: u8 = 66;

/// 把 0..=100 的 pct 映射成 rodio 的线性 gain(0.0..=1.0),走 cubic 感知曲线。
///
/// 人耳响度感大致是 PCM 增益的立方根关系 —— 线性 50% gain 听上去 ≈ "85% 响"。
/// 用 `gain = (pct/100)^3` 反转:UI 显示 50% 时听上去也大约半响,音量条手感才"自然"。
/// Spotify / VLC / Audacious 都用这条。
fn pct_to_gain(pct: u8) -> f32 {
    let p = f32::from(pct.min(100)) / 100.0;
    p * p * p
}

/// stream-download 起播前预拉的字节数。256 KB 在 320 kbps mp3 ≈ 6.4 秒缓冲,
/// seek ±5s 命中已下载区间概率极高,cpal 回调线程不被网络等待阻塞。
const PREFETCH_BYTES: u64 = 256 * 1024;

/// 引擎线程入口。
///
/// `ready_tx` 在引擎完成 sink/runtime 初始化后立刻汇报,UI 才返回 handle。
/// `seek_mailbox` 是与 handle 共享的 latest-wins seek 目标位置。
pub(crate) fn run(
    cmd_rx: &mpsc::Receiver<AudioCommand>,
    snapshot: &Arc<Mutex<AudioSnapshot>>,
    seek_mailbox: &Arc<Mutex<Option<Duration>>>,
    ready_tx: &mpsc::SyncSender<color_eyre::Result<()>>,
    tap_producer: &SharedProd,
    sr_atomic: &Arc<AtomicU32>,
) {
    if let Err(e) = engine_main(
        cmd_rx,
        snapshot,
        seek_mailbox,
        ready_tx,
        tap_producer,
        sr_atomic,
    ) {
        mineral_log::warn!(target: "audio_engine", "exited: {e:?}");
    }
}

fn engine_main(
    cmd_rx: &mpsc::Receiver<AudioCommand>,
    snapshot: &Arc<Mutex<AudioSnapshot>>,
    seek_mailbox: &Arc<Mutex<Option<Duration>>>,
    ready_tx: &mpsc::SyncSender<color_eyre::Result<()>>,
    tap_producer: &SharedProd,
    sr_atomic: &Arc<AtomicU32>,
) -> color_eyre::Result<()> {
    let mut stream_handle = match rodio::DeviceSinkBuilder::open_default_sink() {
        Ok(s) => s,
        Err(e) => {
            let err = eyre!("rodio device sink: {e}");
            let _ = ready_tx.send(Err(eyre!("rodio device sink: {e}")));
            return Err(err);
        }
    };
    // 默认 drop 时会向 stderr 打一行 "Audio playback has finished",TUI 退出后会污染终端,关掉。
    stream_handle.log_on_drop(false);

    let player = rodio::Player::connect_new(stream_handle.mixer());
    player.set_volume(pct_to_gain(DEFAULT_VOLUME_PCT));

    // multi_thread:stream-download 后台下载 task 必须在独立 worker 上持续被 poll,
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
    let mut state = EngineState::default();

    loop {
        match cmd_rx.recv_timeout(TICK) {
            Ok(cmd) => match handle_command(cmd, &player, &rt, &mut state, tap_producer, sr_atomic)
            {
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
        drain_seek(seek_mailbox, &player);
        update_snapshot(snapshot, &player, cur_duration_ms, &mut state);
    }
    Ok(())
}

/// engine 跨 tick 的可变状态。
#[derive(Default)]
struct EngineState {
    /// 是否「正在等一首歌自然结束」。`Play` 时 arm,`Stop` / 检测到自然曲终时 disarm。
    ///
    /// 关键:`player.stop()` 是异步的(rodio 5ms periodic_access 才真停 source),
    /// Stop 命令返回后 sink 仍可能保留几个 ms 的 sound_count > 0。如果靠 `is_empty`
    /// 的裸下降沿,Stop 后那段尾巴退潮会被误判为曲终。armed=false 直接跳过检测。
    armed: bool,

    /// 单调递增的曲终计数,每次自然曲终 +1,写进 snapshot。
    finished_seq: u64,
}

/// 处理一条命令。返回 `Some(duration_ms)` 表示曲目变了,player 现在播这个新曲;
/// `None` 表示 duration 没变(暂停/恢复/音量等)。
fn handle_command(
    cmd: AudioCommand,
    player: &rodio::Player,
    rt: &tokio::runtime::Runtime,
    state: &mut EngineState,
    tap_producer: &SharedProd,
    sr_atomic: &Arc<AtomicU32>,
) -> color_eyre::Result<Option<u64>> {
    match cmd {
        AudioCommand::Play(url) => {
            // 切歌前先 disarm,旧曲尾巴的 sound_count 退潮不会被算成曲终。
            state.armed = false;
            player.stop();
            let dur = append_decoded(player, rt, url, tap_producer, sr_atomic)?;
            player.play();
            // append 内部已 fetch_add 把 sound_count 抬到 1,这里武装后下次 update_snapshot
            // 看到的就是 !is_empty,不存在「armed 后第一 tick 就空」的 race。
            state.armed = true;
            Ok(Some(dur))
        }
        AudioCommand::Pause => {
            player.pause();
            Ok(None)
        }
        AudioCommand::Resume => {
            player.play();
            Ok(None)
        }
        AudioCommand::Stop => {
            // 用户主动 stop 不该触发曲终事件,直接 disarm。
            state.armed = false;
            player.stop();
            Ok(Some(0))
        }
        AudioCommand::SetVolume(pct) => {
            player.set_volume(pct_to_gain(pct));
            Ok(None)
        }
    }
}

/// mailbox 里有 pending seek 就 take 出来打一次 try_seek,latest-wins。
/// 长按 ←/→ 时多次覆写只生效最后一次,避免堆积串行 seek 导致卡顿。
fn drain_seek(seek_mailbox: &Arc<Mutex<Option<Duration>>>, player: &rodio::Player) {
    let Some(target) = seek_mailbox.lock().take() else {
        return;
    };
    if let Err(e) = player.try_seek(target) {
        mineral_log::warn!(target: "audio_engine", "seek to {target:?}: {e}");
    }
}

/// 把 URL 解析成 decoder 并 append 到 player。返回 decoder 探到的 duration(ms,0 = 未知)。
///
/// `tap_producer` / `sr_atomic` 用于把 decoder 包成 [`TapSource`],PCM 旁路写进 ringbuf。
fn append_decoded(
    player: &rodio::Player,
    rt: &tokio::runtime::Runtime,
    url: MediaUrl,
    tap_producer: &SharedProd,
    sr_atomic: &Arc<AtomicU32>,
) -> color_eyre::Result<u64> {
    match url {
        MediaUrl::Remote(u) => {
            let reader = rt.block_on(async {
                let stream = HttpStream::<Client>::create(u)
                    .await
                    .map_err(|e| eyre!("http stream: {e}"))?;
                StreamDownload::from_stream(
                    stream,
                    TempStorageProvider::new(),
                    Settings::default().prefetch_bytes(PREFETCH_BYTES),
                )
                .await
                .map_err(|e| eyre!("stream-download init: {e}"))
            })?;
            let decoder = rodio::Decoder::new(reader).map_err(|e| eyre!("decode: {e}"))?;
            let dur_ms = decoder.total_duration().map(duration_to_ms).unwrap_or(0);
            player.append(TapSource::new(decoder, Arc::clone(tap_producer), sr_atomic));
            Ok(dur_ms)
        }
        MediaUrl::Local(p) => {
            let file = std::fs::File::open(&p).map_err(|e| eyre!("open {}: {e}", p.display()))?;
            let reader = BufReader::new(file);
            let decoder = rodio::Decoder::new(reader).map_err(|e| eyre!("decode: {e}"))?;
            let dur_ms = decoder.total_duration().map(duration_to_ms).unwrap_or(0);
            player.append(TapSource::new(decoder, Arc::clone(tap_producer), sr_atomic));
            Ok(dur_ms)
        }
    }
}

fn update_snapshot(
    snapshot: &Arc<Mutex<AudioSnapshot>>,
    player: &rodio::Player,
    cur_duration_ms: u64,
    state: &mut EngineState,
) {
    let pos_ms = duration_to_ms(player.get_pos());
    let is_paused = player.is_paused();
    let is_empty = player.empty();
    let playing = !is_paused && !is_empty;

    // 仅 armed 时检测「sink 变空」=曲终。armed=false 期间(用户主动 stop / 切歌后未到新 Play)
    // 完全跳过,旧曲尾巴的 sound_count 退潮不会被误判。
    if state.armed && is_empty {
        state.finished_seq += 1;
        state.armed = false;
    }

    let mut g = snapshot.lock();
    g.playing = playing;
    g.position_ms = pos_ms;
    g.duration_ms = cur_duration_ms;
    g.track_finished_seq = state.finished_seq;
    // volume_pct 由 handle.set_volume 直接维护,引擎不反查。
}

fn duration_to_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
