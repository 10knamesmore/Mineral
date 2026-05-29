//! 引擎线程主体:owns rodio device sink + Player + 内嵌 tokio runtime。
//!
//! 命令通道处理 play/pause/resume/stop/set_volume(语义不可合并)。
//! seek 单独走 [`crate::handle::AudioHandle`] → mailbox(latest-wins),engine 每个 tick
//! `take()` 一次实际打 demuxer ——抗住长按 ←/→ 的 30Hz key-repeat。

use std::io::BufReader;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use color_eyre::eyre::eyre;
use mineral_model::MediaUrl;
use parking_lot::Mutex;
use rodio::Source;
use rodio::decoder::DecoderBuilder;
use stream_download::Settings;
use stream_download::StreamDownload;
use stream_download::StreamPhase;
use stream_download::StreamState;
use stream_download::http::HttpStream;
use stream_download::http::reqwest::Client;
use stream_download::source::SourceStream;
use stream_download::storage::temp::TempStorageProvider;

use crate::command::AudioCommand;
use crate::file_storage::FileStorageProvider;
use crate::handle::AudioMode;
use crate::snapshot::{AudioBackend, AudioSnapshot};
use crate::tap::{SharedProd, TapSource};

/// 命令通道空转间隔 / snapshot 刷新节拍 / seek mailbox drain 节拍。
///
/// 20ms 是经验值:OS 键盘 key-repeat 一般 ~30Hz(33ms 一次),20ms tick 能在
/// 用户长按 ←/→ 时把 mailbox 几乎实时 drain → seek,感觉上接近连续(否则两次
/// seek 之间的 tick 间隙会播放旧位置的几十 ms 音频,听感就是「跳一下播一下」)。
/// 同样 stop 命令延迟 ≤20ms,切歌时旧曲尾巴可被压到 cpal 回调缓冲固有长度。
/// termusic 用 5ms,我们留余量。
const TICK: Duration = Duration::from_millis(20);

/// 默认初始音量百分比
const DEFAULT_VOLUME_PCT: u8 = 100;

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

/// 引擎跨线程共享的下载 / 缓冲进度,在 [`engine_main`] 创建一次。下载侧(进度回调 / 完成
/// waiter)写,[`update_snapshot`] 读。代号字段与 [`EngineState::stream_gen`] 比对,挡掉切歌后
/// 旧流的迟到信号。
#[derive(Default)]
struct SharedProgress {
    /// capture 整段下完后 waiter store 的曲目代号(与 `stream_gen` 比对算 `download_complete`)。
    done_gen: AtomicU64,

    /// `buffer_bps` 当前对应的流代号;回调写前先比对,旧流的迟到回调直接 no-op。
    buffer_gen: AtomicU64,

    /// 当前流已缓冲比例(0..=10000 basis points)。
    buffer_bps: AtomicU16,
}

/// 穿过 [`append_decoded`] / [`spawn_remote`] 的进度上下文:当前流代号 + 共享载体 + 是否追踪
/// 整段下完(仅 capture 需要)。把若干进度参数收成一个,避免函数参数数越过 clippy 阈值。
#[derive(Clone, Copy)]
struct ProgressCtx<'a> {
    /// 本次播放的流代号(每次 `Play` / `Stop` 自增)。
    stream_gen: u64,

    /// 共享进度载体。
    shared: &'a Arc<SharedProgress>,

    /// 是否 spawn waiter 等整段下完(capture 播放才需要,用于 `download_complete`)。
    track_completion: bool,
}

/// 已缓冲字节占总字节的比例(0..=10000 basis points)。
///
/// # Params:
///   - `buffered`: 已下载 / 已缓冲的字节数
///   - `total`: 总字节数;`0` 表示长度未知(无 `Content-Length`)
///
/// # Return:
///   basis points,`total == 0` 时返回 `0`,超出 clamp 到满。
fn buffered_bps(buffered: u64, total: u64) -> u16 {
    if total == 0 {
        return 0;
    }
    let bps = buffered.saturating_mul(10_000) / total;
    u16::try_from(bps.min(10_000)).unwrap_or(10_000)
}

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
    mode: AudioMode,
) {
    if let Err(e) = engine_main(
        cmd_rx,
        snapshot,
        seek_mailbox,
        ready_tx,
        tap_producer,
        sr_atomic,
        mode,
    ) {
        mineral_log::error!(target: "audio", error = mineral_log::chain(&e), "engine exited");
    }
}

/// 引擎主循环:初始化 sink/runtime,失败时通过 `ready_tx` 上报,然后循环 recv 命令 + drain seek + 刷 snapshot。
///
/// 无音频设备(或 [`AudioMode::ForceNull`])不算错:置 [`AudioBackend::Null`]、报 ready、进
/// [`run_null_mode`] 空跑——daemon 照常 bind / serve / graceful shutdown,client 据 snapshot 提示降级。
fn engine_main(
    cmd_rx: &mpsc::Receiver<AudioCommand>,
    snapshot: &Arc<Mutex<AudioSnapshot>>,
    seek_mailbox: &Arc<Mutex<Option<Duration>>>,
    ready_tx: &mpsc::SyncSender<color_eyre::Result<()>>,
    tap_producer: &SharedProd,
    sr_atomic: &Arc<AtomicU32>,
    mode: AudioMode,
) -> color_eyre::Result<()> {
    let sink = match mode {
        AudioMode::ForceNull => None,
        AudioMode::Auto => match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(s) => Some(s),
            Err(e) => {
                mineral_log::warn!(
                    target: "audio",
                    error = mineral_log::chain(&eyre!("rodio device sink: {e}")),
                    "no audio device; running in null mode (no sound)"
                );
                None
            }
        },
    };
    let Some(mut stream_handle) = sink else {
        snapshot.lock().backend = AudioBackend::Null;
        let _ = ready_tx.send(Ok(()));
        return run_null_mode(cmd_rx);
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
    // 下载侧共享进度:capture 完成 waiter store 完成代号、流式下载回调写缓冲比例;
    // update_snapshot 按 stream_gen 比对后采信,算 download_complete / buffered_bps。
    let progress = Arc::new(SharedProgress::default());

    loop {
        match cmd_rx.recv_timeout(TICK) {
            Ok(cmd) => match handle_command(
                cmd,
                &player,
                &rt,
                &mut state,
                tap_producer,
                sr_atomic,
                &progress,
            ) {
                Ok(new_dur) => {
                    if let Some(d) = new_dur {
                        cur_duration_ms = d;
                    }
                }
                Err(e) => {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&e), "command error");
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        drain_seek(seek_mailbox, &player);
        update_snapshot(snapshot, &player, cur_duration_ms, &mut state, &progress);
    }
    Ok(())
}

/// 无设备降级循环:没有 sink / runtime,只 drain 命令通道直到发送端全 drop。
///
/// 命令被静默丢弃(无处发声);`set_volume` 等仍由 handle 直接写 snapshot,不依赖此处。
/// 关键是线程**一直活着**,daemon 不会因「audio 起不来」而退出。
fn run_null_mode(cmd_rx: &mpsc::Receiver<AudioCommand>) -> color_eyre::Result<()> {
    // 命令静默丢弃(无 sink 可发声);recv 阻塞到发送端全 drop(daemon 退出)才返回。
    while cmd_rx.recv().is_ok() {}
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

    /// 当前流的代号:每次 `Play`/`Stop` +1。交给下载侧(进度回调 + capture 完成 waiter),
    /// 它们写 [`SharedProgress`] 前先比对此代号,只有「当前这首」的信号被采纳——切歌后旧流
    /// 的迟到回调 / 迟到完成因代号不匹配被挡掉。`update_snapshot` 据此算 `download_complete`
    /// 并采信 `buffer_bps`。
    stream_gen: u64,
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
    progress: &Arc<SharedProgress>,
) -> color_eyre::Result<Option<u64>> {
    match cmd {
        AudioCommand::Play { url, capture } => {
            // 切歌前先 disarm,旧曲尾巴的 sound_count 退潮不会被算成曲终。
            state.armed = false;
            player.stop();
            // 新流 → 新代号:旧流的进度回调 / 完成信号立即失效,download_complete 回落 false。
            state.stream_gen += 1;
            // 起播即把缓冲归零并绑定新代号;远端由下载回调 ramp,本地在 append_decoded 直接置满。
            progress
                .buffer_gen
                .store(state.stream_gen, Ordering::Release);
            progress.buffer_bps.store(0, Ordering::Release);
            let ctx = ProgressCtx {
                stream_gen: state.stream_gen,
                shared: progress,
                track_completion: capture.is_some(),
            };
            let dur = append_decoded(player, rt, url, capture, ctx, tap_producer, sr_atomic)?;
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
            // 换代号,使 download_complete 回落 false;同时清缓冲——没有曲目在播,overlay 应消失。
            state.stream_gen += 1;
            progress
                .buffer_gen
                .store(state.stream_gen, Ordering::Release);
            progress.buffer_bps.store(0, Ordering::Release);
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
        mineral_log::warn!(target: "audio", seek_to = ?target, error = mineral_log::chain(&e), "seek failed");
    }
}

/// 把 URL 解析成 decoder 并 append 到 player。返回 decoder 探到的 duration(ms,0 = 未知)。
///
/// `capture` 非空且 `url` 为 `Remote` 时,用 [`FileStorageProvider`] 把下载字节落到该路径
/// (供播完后入缓存);否则 `Remote` 走会自删的 [`TempStorageProvider`]。
/// `tap_producer` / `sr_atomic` 用于把 decoder 包成 [`TapSource`],PCM 旁路写进 ringbuf。
/// `progress` 把流代号 + 共享进度载体带给远端下载侧;本地播放无下载,直接把缓冲置满。
fn append_decoded(
    player: &rodio::Player,
    rt: &tokio::runtime::Runtime,
    url: MediaUrl,
    capture: Option<std::path::PathBuf>,
    progress: ProgressCtx<'_>,
    tap_producer: &SharedProd,
    sr_atomic: &Arc<AtomicU32>,
) -> color_eyre::Result<u64> {
    mineral_log::info!(target: "audio", url = %url, capture = ?capture, "start decoding");
    match url {
        MediaUrl::Remote(u) => match capture {
            Some(path) => spawn_remote(
                player,
                rt,
                u,
                FileStorageProvider::new(path),
                progress,
                tap_producer,
                sr_atomic,
            ),
            None => spawn_remote(
                player,
                rt,
                u,
                TempStorageProvider::new(),
                progress,
                tap_producer,
                sr_atomic,
            ),
        },
        MediaUrl::Local(p) => {
            let file = std::fs::File::open(&p).map_err(|e| eyre!("open {}: {e}", p.display()))?;
            let byte_len = file.metadata().ok().map(|m| m.len());
            let reader = BufReader::new(file);
            let decoder = build_decoder(reader, byte_len)?;
            let dur_ms = decoder.total_duration().map(duration_to_ms).unwrap_or(0);
            player.append(TapSource::new(decoder, Arc::clone(tap_producer), sr_atomic));
            // 本地文件无网络下载:整段立即可播,缓冲恒满(代号已在 Play 时绑定)。
            progress.shared.buffer_bps.store(10_000, Ordering::Release);
            Ok(dur_ms)
        }
    }
}

/// 用给定 `StorageProvider` 起 stream-download、构 decoder 并 append。`Remote` 两种 provider
/// (会自删的 temp / 持久 capture)走同一条泛型路径,差别只在 `provider` 一个参数。
///
/// # Params:
///   - `url`: 远端音频 URL
///   - `provider`: stream-download 的存储后端
///
/// # Return:
///   decoder 探到的 duration(ms,0 = 未知)。
fn spawn_remote<P>(
    player: &rodio::Player,
    rt: &tokio::runtime::Runtime,
    url: url::Url,
    provider: P,
    progress: ProgressCtx<'_>,
    tap_producer: &SharedProd,
    sr_atomic: &Arc<AtomicU32>,
) -> color_eyre::Result<u64>
where
    P: stream_download::storage::StorageProvider + 'static,
    P::Reader: std::io::Read + std::io::Seek + Send + Sync + 'static,
{
    let stream_gen = progress.stream_gen;
    // 缓冲回调用:每来一块就把「已下字节 / 总字节」写进共享进度。content_length 在 stream
    // 建好后、from_stream 之前才知道,故 Settings 必须在 async 块内、拿到 len 之后构造。
    let buffer = Arc::clone(progress.shared);
    let (reader, byte_len) = rt.block_on(async move {
        let stream = HttpStream::<Client>::create(url)
            .await
            .map_err(|e| eyre!("http stream: {e}"))?;
        let len = stream.content_length();
        let total = len.unwrap_or(0);
        let settings = Settings::default()
            .prefetch_bytes(PREFETCH_BYTES)
            .on_progress(
                move |_stream: &HttpStream<Client>, state: StreamState, _cancel| {
                    // 切歌后旧流的迟到回调(代号不匹配)直接忽略,不污染当前曲缓冲。
                    if buffer.buffer_gen.load(Ordering::Acquire) != stream_gen {
                        return;
                    }
                    let bps = match state.phase {
                        // 长度未知(无 Content-Length)时 buffered_bps 恒 0,下完瞬间补满。
                        StreamPhase::Complete => 10_000,
                        _ => buffered_bps(state.current_position, total),
                    };
                    buffer.buffer_bps.store(bps, Ordering::Release);
                },
            );
        let reader = StreamDownload::from_stream(stream, provider, settings)
            .await
            .map_err(|e| eyre!("stream-download init: {e}"))?;
        Ok::<_, color_eyre::Report>((reader, len))
    })?;
    // capture 播放:拿 download handle,spawn 一个 waiter 等整段下完后 store 本曲代号。
    // 必须在 reader 被 decoder 消费前取 handle。
    if progress.track_completion {
        let handle = reader.handle();
        let done = Arc::clone(progress.shared);
        rt.spawn(async move {
            handle.wait_for_completion().await;
            done.done_gen.store(stream_gen, Ordering::Release);
        });
    }
    let decoder = build_decoder(reader, byte_len)?;
    let dur_ms = decoder.total_duration().map(duration_to_ms).unwrap_or(0);
    player.append(TapSource::new(decoder, Arc::clone(tap_producer), sr_atomic));
    Ok(dur_ms)
}

/// 用 [`DecoderBuilder`] 构造 decoder,**`byte_len` 已知时一并塞进**。
///
/// 关键:rodio `Decoder::new()` 默认 `is_seekable=false`,Symphonia 在源不可
/// 随机访问时只能向前 seek(后退会返 `ForwardOnly` → `RandomAccessNotSupported`)
/// —— 表现就是按 ← 没反应。`with_byte_len` 会一并把 `is_seekable` 置 true。
/// `byte_len` 未知时退化到默认行为(只能向前 seek),至少不比之前差。
fn build_decoder<R>(reader: R, byte_len: Option<u64>) -> color_eyre::Result<rodio::Decoder<R>>
where
    R: std::io::Read + std::io::Seek + Send + Sync + 'static,
{
    let mut builder = DecoderBuilder::new().with_data(reader);
    if let Some(len) = byte_len {
        builder = builder.with_byte_len(len);
    }
    builder.build().map_err(|e| eyre!("decode: {e}"))
}

/// 把 player 当前播放状态拍进共享 snapshot,顺带在 armed 状态下检测「sink 变空 → 曲终」。
fn update_snapshot(
    snapshot: &Arc<Mutex<AudioSnapshot>>,
    player: &rodio::Player,
    cur_duration_ms: u64,
    state: &mut EngineState,
    progress: &Arc<SharedProgress>,
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

    // 当前曲的 capture 下载完成 = waiter 已 store 本曲代号。代号不匹配(旧曲迟到 / 非 capture)→ false。
    let download_complete =
        state.stream_gen != 0 && progress.done_gen.load(Ordering::Acquire) == state.stream_gen;

    let mut g = snapshot.lock();
    g.playing = playing;
    g.position_ms = pos_ms;
    g.duration_ms = cur_duration_ms;
    g.track_finished_seq = state.finished_seq;
    g.download_complete = download_complete;
    // 下载侧的进度回调按当前代号写入,这里直接采信(切歌时已在 Play/Stop 归零)。
    g.buffered_bps = progress.buffer_bps.load(Ordering::Acquire);
    // volume_pct 由 handle.set_volume 直接维护,引擎不反查。
}

/// `Duration` → ms,超过 `u64::MAX` 时饱和(实际曲长不会触达)。
fn duration_to_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::buffered_bps;

    /// `buffered_bps`:0 / 一半 / 满 / 超界 clamp;`total == 0`(长度未知)恒 0。
    #[test]
    fn buffered_bps_cases() {
        assert_eq!(buffered_bps(0, 1000), 0);
        assert_eq!(buffered_bps(500, 1000), 5_000);
        assert_eq!(buffered_bps(1000, 1000), 10_000);
        // 已下超过总长(理论不该发生)clamp 到满,不溢出。
        assert_eq!(buffered_bps(2000, 1000), 10_000);
        // 长度未知:无法算比例,返回 0(由完成回调在 Complete 时补满)。
        assert_eq!(buffered_bps(123, 0), 0);
        assert_eq!(buffered_bps(0, 0), 0);
    }

    /// 极大字节数不因 `* 10_000` 溢出 / panic,结果始终 ≤ 满格;现实 GB 量级整段缓冲 = 满格。
    #[test]
    fn buffered_bps_no_overflow_on_huge_bytes() {
        // 病态量级(saturating_mul 兜底,不 panic);具体值无意义,只要 clamp 在范围内。
        assert!(buffered_bps(u64::MAX, u64::MAX) <= 10_000);
        assert!(buffered_bps(u64::MAX, 1) <= 10_000);
        // 现实量级(2 GB)整段下完 = 满格,saturating 不触发。
        assert_eq!(buffered_bps(2_000_000_000, 2_000_000_000), 10_000);
    }
}
