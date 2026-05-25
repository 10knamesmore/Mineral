//! 引擎线程主体:owns rodio device sink + Player + 内嵌 tokio runtime。
//!
//! 命令通道处理 play/pause/resume/stop/set_volume(语义不可合并)。
//! seek 单独走 [`crate::handle::AudioHandle`] → mailbox(latest-wins),engine 每个 tick
//! `take()` 一次实际打 demuxer ——抗住长按 ←/→ 的 30Hz key-repeat。

use std::io::BufReader;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use color_eyre::eyre::eyre;
use mineral_model::MediaUrl;
use parking_lot::Mutex;
use rodio::Source;
use rodio::decoder::DecoderBuilder;
use stream_download::Settings;
use stream_download::StreamDownload;
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
    // capture 下载完成标记:waiter task 在 `wait_for_completion` 后 store 该曲的 gen;
    // update_snapshot 比对当前 gen 算出 download_complete。
    let download_done_gen = Arc::new(AtomicU64::new(0));

    loop {
        match cmd_rx.recv_timeout(TICK) {
            Ok(cmd) => match handle_command(
                cmd,
                &player,
                &rt,
                &mut state,
                tap_producer,
                sr_atomic,
                &download_done_gen,
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
        update_snapshot(
            snapshot,
            &player,
            cur_duration_ms,
            &mut state,
            &download_done_gen,
        );
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

    /// 当前曲目的 capture 代号:每次 `Play`/`Stop` +1。capture 播放时把它交给 waiter;
    /// waiter 在下载完成后 store 进 `download_done_gen`。`update_snapshot` 比对二者算
    /// `download_complete`,从而只认「当前这首」的下载完成,旧曲的迟到信号被 gen 不匹配挡掉。
    capture_gen: u64,
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
    download_done_gen: &Arc<AtomicU64>,
) -> color_eyre::Result<Option<u64>> {
    match cmd {
        AudioCommand::Play { url, capture } => {
            // 切歌前先 disarm,旧曲尾巴的 sound_count 退潮不会被算成曲终。
            state.armed = false;
            player.stop();
            // 新曲 → 新 gen(让上一首的 download_complete 立即回落 false)。
            state.capture_gen += 1;
            let completion = capture
                .is_some()
                .then(|| (state.capture_gen, Arc::clone(download_done_gen)));
            let dur = append_decoded(
                player,
                rt,
                url,
                capture,
                completion,
                tap_producer,
                sr_atomic,
            )?;
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
            // 换 gen,使 download_complete 回落 false(没有曲目在播)。
            state.capture_gen += 1;
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
fn append_decoded(
    player: &rodio::Player,
    rt: &tokio::runtime::Runtime,
    url: MediaUrl,
    capture: Option<std::path::PathBuf>,
    completion: Option<(u64, Arc<AtomicU64>)>,
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
                completion,
                tap_producer,
                sr_atomic,
            ),
            None => spawn_remote(
                player,
                rt,
                u,
                TempStorageProvider::new(),
                /*completion*/ None,
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
    completion: Option<(u64, Arc<AtomicU64>)>,
    tap_producer: &SharedProd,
    sr_atomic: &Arc<AtomicU32>,
) -> color_eyre::Result<u64>
where
    P: stream_download::storage::StorageProvider + 'static,
    P::Reader: std::io::Read + std::io::Seek + Send + Sync + 'static,
{
    let (reader, byte_len) = rt.block_on(async {
        let stream = HttpStream::<Client>::create(url)
            .await
            .map_err(|e| eyre!("http stream: {e}"))?;
        let len = stream.content_length();
        let reader = StreamDownload::from_stream(
            stream,
            provider,
            Settings::default().prefetch_bytes(PREFETCH_BYTES),
        )
        .await
        .map_err(|e| eyre!("stream-download init: {e}"))?;
        Ok::<_, color_eyre::Report>((reader, len))
    })?;
    // capture 播放:拿 download handle,spawn 一个 waiter 等整段下完后 store 本曲 gen。
    // 必须在 reader 被 decoder 消费前取 handle。
    if let Some((track_gen, done_gen)) = completion {
        let handle = reader.handle();
        rt.spawn(async move {
            handle.wait_for_completion().await;
            done_gen.store(track_gen, Ordering::Release);
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
    download_done_gen: &Arc<AtomicU64>,
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

    // 当前曲的 capture 下载完成 = waiter 已 store 本曲 gen。gen 不匹配(旧曲迟到 / 非 capture)→ false。
    let download_complete =
        state.capture_gen != 0 && download_done_gen.load(Ordering::Acquire) == state.capture_gen;

    let mut g = snapshot.lock();
    g.playing = playing;
    g.position_ms = pos_ms;
    g.duration_ms = cur_duration_ms;
    g.track_finished_seq = state.finished_seq;
    g.download_complete = download_complete;
    // volume_pct 由 handle.set_volume 直接维护,引擎不反查。
}

/// `Duration` → ms,超过 `u64::MAX` 时饱和(实际曲长不会触达)。
fn duration_to_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
