//! 引擎线程主体:owns rodio device sink + Player + 内嵌 tokio runtime。
//!
//! 命令通道处理 play/append_next/clear_next/pause/resume/stop/set_volume(语义不可合并)。
//! seek 单独走 [`crate::handle::AudioHandle`] → mailbox(latest-wins),engine 每个 tick
//! `take()` 一次实际打 demuxer ——抗住长按 ←/→ 的 30Hz key-repeat。
//!
//! gapless:除「当前曲」外可多排一首「下一曲」decoder 进 rodio 队列([`crate::queue_slots`]
//! 的 [`PlayHead`] 记账),当前曲自然耗尽时 rodio 零静音接续。预排远端曲的建流 / 预缓冲在
//! runtime 上**链下**进行,就绪后经通道交回引擎线程 build decoder + `append`,不阻塞命令线程。

use std::io::{BufReader, Read, Seek};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
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
use stream_download::storage::StorageProvider;
use stream_download::storage::temp::TempStorageProvider;

use crate::command::AudioCommand;
use crate::file_storage::FileStorageProvider;
use crate::handle::AudioMode;
use crate::queue_slots::{Boundary, PlayHead, SharedProgress, Slot};
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

/// `Read + Seek + Send + Sync` 的对象安全别名:把不同 `StorageProvider` 的 reader
/// (远端流 / 本地文件)装箱成同一类型,链下建好的下一曲 reader 经统一通道交回引擎线程。
///
/// `Box<dyn ReadSeek>` 经 std 的 `impl<R: Read+?Sized> Read for Box<R>`(Seek 同理)自动
/// 获得 Read/Seek(`dyn ReadSeek` 含超 trait),无需手写转发 impl。
trait ReadSeek: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> ReadSeek for T {}

/// 链下建好的下一曲:reader 已就绪(预缓冲完成),交回引擎线程 build decoder + append。
struct NextBuilt {
    /// 已就绪的装箱 reader(远端 StreamDownload / 本地 BufReader)。
    reader: Box<dyn ReadSeek>,

    /// 已知字节长度(供 decoder seekable;`None` 仅能向前 seek)。
    byte_len: Option<u64>,

    /// 本次预排的流代号(与 `pending_next_gen` 比对,挡掉 ClearNext / 被新预排取代的迟到结果)。
    stream_gen: u64,

    /// 本曲占用的进度槽下标(0/1)。
    progress_idx: usize,

    /// 本地源:无网络下载,append 后直接把缓冲置满。
    local_full: bool,
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

/// 引擎主循环:初始化 sink/runtime,失败时通过 `ready_tx` 上报,然后循环 recv 命令 +
/// drain 链下建好的下一曲 + drain seek + 刷 snapshot。
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
    // 3 个 worker:gapless 预排会让当前曲 + 下一曲两路下载并发,留一个余量。
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
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

    let mut engine = Engine::new(&player, &rt, tap_producer, sr_atomic);

    loop {
        match cmd_rx.recv_timeout(TICK) {
            Ok(cmd) => engine.handle_command(cmd),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        engine.drain_next_built();
        drain_seek(seek_mailbox, &player);
        engine.update_snapshot(snapshot);
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

/// 引擎跨 tick 的可变状态 + 不可变依赖(player / rt / tap / 进度载体 / 链下结果通道)。
struct Engine<'a> {
    /// rodio 播放器(队列)。
    player: &'a rodio::Player,

    /// 内嵌 tokio runtime(远端建流 / 后台下载)。
    rt: &'a tokio::runtime::Runtime,

    /// PCM tap 共享写端(每首曲目包一个 [`TapSource`])。
    tap_producer: SharedProd,

    /// 当前出声曲目的采样率原子(UI spectrum 读);在起播 / 边界轮转时精确写。
    sr_atomic: Arc<AtomicU32>,

    /// 双槽共享下载 / 缓冲进度。
    progress: Arc<SharedProgress>,

    /// 链下建好的下一曲发送端(spawn 任务用)。
    next_built_tx: mpsc::Sender<NextBuilt>,

    /// 链下建好的下一曲接收端(引擎主循环每 tick drain)。
    next_built_rx: mpsc::Receiver<NextBuilt>,

    /// 2-slot 播放头(当前曲 + 已预排下一曲的记账、边界推进)。
    head: PlayHead,

    /// 单调流代号种子:每次 Play / AppendNext 自增分配,全局唯一不复用。
    gen_seq: u64,

    /// 当前等待链下结果的预排代号(0 = 无);ClearNext / 被新预排取代时作废。
    pending_next_gen: u64,

    /// 当前曲采样率(边界轮转时由 `next_sample_rate` 顶上,写进 `sr_atomic`)。
    cur_sample_rate: u32,

    /// 已预排下一曲的采样率(append 时记下,等轮转成当前曲才写 `sr_atomic`)。
    next_sample_rate: u32,
}

impl<'a> Engine<'a> {
    /// 构造引擎状态。
    fn new(
        player: &'a rodio::Player,
        rt: &'a tokio::runtime::Runtime,
        tap_producer: &SharedProd,
        sr_atomic: &Arc<AtomicU32>,
    ) -> Self {
        let (next_built_tx, next_built_rx) = mpsc::channel();
        Self {
            player,
            rt,
            tap_producer: Arc::clone(tap_producer),
            sr_atomic: Arc::clone(sr_atomic),
            progress: Arc::new(SharedProgress::default()),
            next_built_tx,
            next_built_rx,
            head: PlayHead::default(),
            gen_seq: 0,
            pending_next_gen: 0,
            cur_sample_rate: 0,
            next_sample_rate: 0,
        }
    }

    /// 分配一个新的全局唯一流代号。
    fn next_gen(&mut self) -> u64 {
        self.gen_seq += 1;
        self.gen_seq
    }

    /// 处理一条命令。错误就地 warn,不冒泡(单命令失败不该掀掉引擎线程)。
    fn handle_command(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::Play { url, capture } => {
                if let Err(e) = self.play(url, capture) {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&e), "play error");
                }
            }
            AudioCommand::AppendNext { url, capture } => self.append_next(url, capture),
            AudioCommand::ClearNext => self.clear_next(),
            AudioCommand::Pause => self.player.pause(),
            AudioCommand::Resume => self.player.play(),
            AudioCommand::Stop => self.stop(),
            AudioCommand::SetVolume(pct) => self.player.set_volume(pct_to_gain(pct)),
        }
    }

    /// 切到 `url` 从头播(cut-over):停掉当前队列(含已预排的 next)、作废待建预排,
    /// 解码 + append 新曲后起播,武装 [`PlayHead`] 当前槽。
    fn play(&mut self, url: MediaUrl, capture: Option<PathBuf>) -> color_eyre::Result<()> {
        mineral_log::info!(target: "audio", url = %url, capture = ?capture, "start decoding");
        // 切歌前先 disarm,旧曲尾巴(及已预排 next)的 sound_count 退潮不会被算成曲终。
        self.head.cur.occupied = false;
        self.head.next.occupied = false;
        self.player.stop();
        self.pending_next_gen = 0;
        // 切歌即让采样率失效:流式 build 阻塞期间 snapshot 不刷新,不清零会残留上一首采样率
        // 直到本首 decoder 建好后才更新。此刻已 stop、无 PCM 喂频谱,清零安全。
        self.cur_sample_rate = 0;
        self.sr_atomic.store(0, Ordering::Relaxed);

        let track_gen = self.next_gen();
        let idx = 0;
        self.reset_progress(idx, track_gen);
        let (dur_ms, sr, local) = self.build_and_append_blocking(url, capture, track_gen, idx)?;
        if local {
            self.progress
                .slot(idx)
                .buffer_bps
                .store(10_000, Ordering::Release);
        }
        self.player.play();
        self.cur_sample_rate = sr;
        self.sr_atomic.store(sr, Ordering::Relaxed);
        // append 内部已 fetch_add 把 sound_count 抬到 1,武装后下次 update_snapshot 看到的就是
        // 已占用,不存在「武装后第一 tick 就空」的 race。
        self.head.start(Slot {
            stream_gen: track_gen,
            duration_ms: dur_ms,
            progress_idx: idx,
            occupied: true,
        });
        Ok(())
    }

    /// 预排下一曲:占用另一进度槽,远端走链下建流(就绪后 drain 才 append),本地立即排进通道。
    /// 当前无曲在播时忽略(上层不该在停止态预排)。
    fn append_next(&mut self, url: MediaUrl, capture: Option<PathBuf>) {
        if !self.head.cur.occupied {
            return;
        }
        if self.head.next.occupied {
            // 已有预排;上层应先消化边界再排。忽略以免双排。
            return;
        }
        let track_gen = self.next_gen();
        let idx = if self.head.cur.progress_idx == 0 {
            1
        } else {
            0
        };
        self.reset_progress(idx, track_gen);
        self.pending_next_gen = track_gen;
        mineral_log::debug!(target: "audio", url = %url, stream_gen = track_gen, "append next (prefetch)");
        match url {
            MediaUrl::Remote(u) => {
                let tx = self.next_built_tx.clone();
                let progress = Arc::clone(&self.progress);
                self.rt.spawn(async move {
                    match create_stream(u, capture, track_gen, idx, progress).await {
                        Ok((reader, byte_len)) => {
                            let _ = tx.send(NextBuilt {
                                reader,
                                byte_len,
                                stream_gen: track_gen,
                                progress_idx: idx,
                                local_full: false,
                            });
                        }
                        Err(e) => {
                            mineral_log::warn!(target: "audio", error = mineral_log::chain(&e), "prefetch next stream failed");
                        }
                    }
                });
            }
            MediaUrl::Local(p) => match open_local(&p) {
                Ok((reader, byte_len)) => {
                    let _ = self.next_built_tx.send(NextBuilt {
                        reader,
                        byte_len,
                        stream_gen: track_gen,
                        progress_idx: idx,
                        local_full: true,
                    });
                }
                Err(e) => {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&e), "prefetch next local open failed");
                }
            },
        }
    }

    /// 撤销「尚未 append」的待建下一曲:作废 pending 代号,迟到的链下结果按代号丢弃。
    /// 已 append(next 槽占用)则无法从 rodio 队列摘除(仅 next 未就绪时才会被调),no-op。
    fn clear_next(&mut self) {
        if self.head.next.occupied {
            mineral_log::debug!(target: "audio", "clear_next: 已 append,无法撤销");
            return;
        }
        self.pending_next_gen = 0;
    }

    /// 用户主动停止:disarm 当前 + 已预排,停队列,作废待建预排。snapshot 经槽未占用自动回落
    /// (buffered / download_complete / duration 归零),无需手动清。
    fn stop(&mut self) {
        self.head.cur.occupied = false;
        self.head.next.occupied = false;
        self.player.stop();
        self.pending_next_gen = 0;
    }

    /// 复位某进度槽:绑定新代号、缓冲归零。`done_gen` 不复位——代号单调不复用,旧值永不等于新代号。
    fn reset_progress(&self, idx: usize, track_gen: u64) {
        let slot = self.progress.slot(idx);
        slot.buffer_gen.store(track_gen, Ordering::Release);
        slot.buffer_bps.store(0, Ordering::Release);
    }

    /// 同步(block_on)建流 + build decoder + append 当前曲(cut-over 的有意阻塞,沿用历史行为)。
    ///
    /// # Return:
    ///   `(duration_ms, sample_rate, is_local)`。
    fn build_and_append_blocking(
        &self,
        url: MediaUrl,
        capture: Option<PathBuf>,
        track_gen: u64,
        idx: usize,
    ) -> color_eyre::Result<(u64, u32, bool)> {
        let (reader, byte_len, local) = match url {
            MediaUrl::Remote(u) => {
                let progress = Arc::clone(&self.progress);
                let (reader, byte_len) = self
                    .rt
                    .block_on(create_stream(u, capture, track_gen, idx, progress))?;
                (reader, byte_len, false)
            }
            MediaUrl::Local(p) => {
                let (reader, byte_len) = open_local(&p)?;
                (reader, byte_len, true)
            }
        };
        let decoder = build_decoder(reader, byte_len)?;
        let dur_ms = decoder.total_duration().map(duration_to_ms).unwrap_or(0);
        let sr = u32::from(decoder.sample_rate());
        // 采样率 / byte_len 入日志:曲间采样率不一致或 byte_len 缺失会影响无缝衔接,便于现场排查。
        mineral_log::info!(
            target: "audio", slot = "cur", sample_rate = sr, dur_ms,
            byte_len_known = byte_len.is_some(), "decoder ready"
        );
        self.player
            .append(TapSource::new(decoder, Arc::clone(&self.tap_producer)));
        Ok((dur_ms, sr, local))
    }

    /// drain 链下建好的下一曲:仍是当前等待的预排、且当前在播、next 槽空 → build decoder + append +
    /// 武装 next 槽;代号不匹配(ClearNext / 被取代)直接丢弃,其 reader drop 即取消后台下载。
    fn drain_next_built(&mut self) {
        while let Ok(built) = self.next_built_rx.try_recv() {
            if built.stream_gen != self.pending_next_gen
                || !self.head.cur.occupied
                || self.head.next.occupied
            {
                continue;
            }
            let byte_len_known = built.byte_len.is_some();
            match build_decoder(built.reader, built.byte_len) {
                Ok(decoder) => {
                    let dur_ms = decoder.total_duration().map(duration_to_ms).unwrap_or(0);
                    self.next_sample_rate = u32::from(decoder.sample_rate());
                    // 预排曲同样记采样率 / byte_len(便于排查曲间衔接)。
                    mineral_log::info!(
                        target: "audio", slot = "next", sample_rate = self.next_sample_rate,
                        dur_ms, byte_len_known, "decoder ready (prefetch)"
                    );
                    self.player
                        .append(TapSource::new(decoder, Arc::clone(&self.tap_producer)));
                    if built.local_full {
                        self.progress
                            .slot(built.progress_idx)
                            .buffer_bps
                            .store(10_000, Ordering::Release);
                    }
                    self.head.arm_next(Slot {
                        stream_gen: built.stream_gen,
                        duration_ms: dur_ms,
                        progress_idx: built.progress_idx,
                        occupied: true,
                    });
                    self.pending_next_gen = 0;
                }
                Err(e) => {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&e), "prefetch next decode failed");
                    self.pending_next_gen = 0;
                }
            }
        }
    }

    /// 把 player 当前播放状态拍进共享 snapshot,顺带观测 `len()` 推进 [`PlayHead`] 边界。
    fn update_snapshot(&mut self, snapshot: &Arc<Mutex<AudioSnapshot>>) {
        let pos_ms = duration_to_ms(self.player.get_pos());
        let is_paused = self.player.is_paused();
        let boundary = self.head.observe(self.player.len());
        if boundary == Boundary::Gapless {
            // 下一曲已轮转成当前曲:此刻才把采样率切过去,频谱不提前跳。
            self.cur_sample_rate = self.next_sample_rate;
            self.sr_atomic
                .store(self.cur_sample_rate, Ordering::Relaxed);
        }
        let playing = !is_paused && self.head.cur.occupied;
        let f = self.head.snapshot_fields(&self.progress);

        let mut g = snapshot.lock();
        g.playing = playing;
        g.position_ms = pos_ms;
        g.duration_ms = f.duration_ms;
        g.track_finished_seq = f.track_finished_seq;
        g.current_track_token = f.current_track_token;
        g.download_complete = f.download_complete;
        g.buffered_bps = f.buffered_bps;
        g.next_duration_ms = f.next_duration_ms;
        g.next_buffered_bps = f.next_buffered_bps;
        g.next_ready = f.next_ready;
        g.next_download_complete = f.next_download_complete;
        g.sample_rate_hz = self.cur_sample_rate;
        // volume_pct 由 handle.set_volume 直接维护,引擎不反查。
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

/// 打开本地文件成装箱 reader(+ 已知字节长度,供 decoder seekable)。
fn open_local(p: &std::path::Path) -> color_eyre::Result<(Box<dyn ReadSeek>, Option<u64>)> {
    let file = std::fs::File::open(p).map_err(|e| eyre!("open {}: {e}", p.display()))?;
    let byte_len = file.metadata().ok().map(|m| m.len());
    Ok((Box::new(BufReader::new(file)), byte_len))
}

/// 起 stream-download(远端):建 HTTP 流、装好缓冲进度回调(写指定进度槽,代号门控挡旧流)、
/// capture(非空)时 spawn 完成 waiter store 下完代号,返回装箱 reader + 字节长度。
///
/// # Params:
///   - `url`: 远端音频 URL
///   - `capture`: 落盘路径(`Some` = 持久 capture 供入缓存,`None` = 会自删的 temp)
///   - `stream_gen`: 本流代号
///   - `progress_idx`: 写入的进度槽下标
///   - `progress`: 双槽共享进度
///
/// # Return:
///   `(装箱 reader, 字节长度)`;字节长度 `None` 表示无 `Content-Length`。
async fn create_stream(
    url: url::Url,
    capture: Option<PathBuf>,
    stream_gen: u64,
    progress_idx: usize,
    progress: Arc<SharedProgress>,
) -> color_eyre::Result<(Box<dyn ReadSeek>, Option<u64>)> {
    match capture {
        Some(path) => {
            stream_with_provider(
                url,
                FileStorageProvider::new(path),
                stream_gen,
                progress_idx,
                progress,
                /*track_completion*/ true,
            )
            .await
        }
        None => {
            stream_with_provider(
                url,
                TempStorageProvider::new(),
                stream_gen,
                progress_idx,
                progress,
                /*track_completion*/ false,
            )
            .await
        }
    }
}

/// 用给定 `StorageProvider` 起 stream-download(两种 provider 走同一泛型路径,差别只在 `provider`)。
///
/// # Params:
///   - `track_completion`: 是否 spawn waiter 等整段下完(capture 才需,用于 `download_complete`)
async fn stream_with_provider<P>(
    url: url::Url,
    provider: P,
    stream_gen: u64,
    progress_idx: usize,
    progress: Arc<SharedProgress>,
    track_completion: bool,
) -> color_eyre::Result<(Box<dyn ReadSeek>, Option<u64>)>
where
    P: StorageProvider + 'static,
    P::Reader: Read + Seek + Send + Sync + 'static,
{
    let stream = HttpStream::<Client>::create(url)
        .await
        .map_err(|e| eyre!("http stream: {e}"))?;
    let len = stream.content_length();
    let total = len.unwrap_or(0);
    let prog = Arc::clone(&progress);
    let settings = Settings::default()
        .prefetch_bytes(PREFETCH_BYTES)
        .on_progress(
            move |_stream: &HttpStream<Client>, state: StreamState, _cancel| {
                // 切歌 / 换预排后旧流的迟到回调(代号不匹配)直接忽略,不污染当前缓冲。
                let slot = prog.slot(progress_idx);
                if slot.buffer_gen.load(Ordering::Acquire) != stream_gen {
                    return;
                }
                let bps = match state.phase {
                    // 长度未知(无 Content-Length)时 buffered_bps 恒 0,下完瞬间补满。
                    StreamPhase::Complete => 10_000,
                    _ => buffered_bps(state.current_position, total),
                };
                slot.buffer_bps.store(bps, Ordering::Release);
            },
        );
    let reader = StreamDownload::from_stream(stream, provider, settings)
        .await
        .map_err(|e| eyre!("stream-download init: {e}"))?;
    // capture 播放:拿 download handle,spawn 一个 waiter 等整段下完后 store 本曲代号。
    // 必须在 reader 被 decoder 消费前取 handle。
    if track_completion {
        let handle = reader.handle();
        let done = Arc::clone(&progress);
        tokio::spawn(async move {
            handle.wait_for_completion().await;
            done.slot(progress_idx)
                .done_gen
                .store(stream_gen, Ordering::Release);
        });
    }
    Ok((Box::new(reader), len))
}

/// 用 [`DecoderBuilder`] 构造 decoder,**`byte_len` 已知时一并塞进**。
///
/// 关键:rodio `Decoder::new()` 默认 `is_seekable=false`,Symphonia 在源不可
/// 随机访问时只能向前 seek(后退会返 `ForwardOnly` → `RandomAccessNotSupported`)
/// —— 表现就是按 ← 没反应。`with_byte_len` 会一并把 `is_seekable` 置 true。
/// `byte_len` 未知时退化到默认行为(只能向前 seek),至少不比之前差。
fn build_decoder<R>(reader: R, byte_len: Option<u64>) -> color_eyre::Result<rodio::Decoder<R>>
where
    R: Read + Seek + Send + Sync + 'static,
{
    let mut builder = DecoderBuilder::new().with_data(reader);
    if let Some(len) = byte_len {
        builder = builder.with_byte_len(len);
    }
    builder.build().map_err(|e| eyre!("decode: {e}"))
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
