//! UI 持有的 audio handle:线程安全、可 clone,所有方法都是非阻塞。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use color_eyre::eyre::eyre;
use mineral_model::MediaUrl;
use parking_lot::Mutex;
use ringbuf::traits::{Consumer, Split};
use ringbuf::{HeapCons, HeapRb};

use crate::command::AudioCommand;
use crate::engine;
use crate::snapshot::AudioSnapshot;
use crate::tap::SharedProd;

/// PCM tap ringbuf 容量(f32 样本)。覆盖 2 个 2048-sample FFT 窗,UI 慢一帧不丢窗。
const TAP_CAPACITY: usize = 4096;

/// 共享内部状态:命令通道 + snapshot + seek mailbox(latest-wins)。
struct Inner {
    cmd_tx: mpsc::Sender<AudioCommand>,
    snapshot: Arc<Mutex<AudioSnapshot>>,
    seek_mailbox: Arc<Mutex<Option<Duration>>>,
}

/// PCM tap:UI 端独占,持有 ringbuf 读端 + 当前轨道 sample_rate。
///
/// SPSC 的 consumer 不能 clone,所以 tap 跟 [`AudioHandle`] 拆开返回 ——
/// handle 仍可任意 clone 给多处调命令,tap 只能在一处(spectrum tick)用。
pub struct SpectrumTap {
    /// ringbuf 读端。
    consumer: HeapCons<f32>,

    /// 当前轨道采样率(Hz),engine 在每首歌包 TapSource 时写入。
    sample_rate: Arc<AtomicU32>,
}

impl SpectrumTap {
    /// 把可读样本拉进 `dst`,返回实际写入数。`dst` 大于环内可读数时只写部分。
    pub fn pop_into(&mut self, dst: &mut [f32]) -> usize {
        self.consumer.pop_slice(dst)
    }

    /// 当前轨道的采样率(Hz)。0 = 还没开始播。
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }
}

/// 音频引擎对外句柄。clone 廉价,跨线程安全。
#[derive(Clone)]
pub struct AudioHandle {
    inner: Arc<Inner>,
}

impl AudioHandle {
    /// 启动 engine 线程并返回 (handle, spectrum tap)。
    /// 失败通常意味着默认输出设备不可用。
    pub fn spawn() -> color_eyre::Result<(Self, SpectrumTap)> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<AudioCommand>();
        let snapshot = Arc::new(Mutex::new(AudioSnapshot {
            volume_pct: 66,
            ..AudioSnapshot::default()
        }));

        let seek_mailbox = Arc::new(Mutex::new(None::<Duration>));

        let rb = HeapRb::<f32>::new(TAP_CAPACITY);
        let (producer, consumer) = rb.split();
        let shared_prod: SharedProd = Arc::new(Mutex::new(producer));
        let sr_atomic = Arc::new(AtomicU32::new(0));

        let (ready_tx, ready_rx) = mpsc::sync_channel::<color_eyre::Result<()>>(1);
        let snap_for_engine = Arc::clone(&snapshot);
        let mailbox_for_engine = Arc::clone(&seek_mailbox);
        let prod_for_engine = Arc::clone(&shared_prod);
        let sr_for_engine = Arc::clone(&sr_atomic);
        thread::Builder::new()
            .name(String::from("mineral-audio"))
            .spawn(move || {
                engine::run(
                    &cmd_rx,
                    &snap_for_engine,
                    &mailbox_for_engine,
                    &ready_tx,
                    &prod_for_engine,
                    &sr_for_engine,
                );
            })
            .map_err(|e| eyre!("spawn audio thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(eyre!("audio engine startup channel: {e}")),
        }

        let handle = Self {
            inner: Arc::new(Inner {
                cmd_tx,
                snapshot,
                seek_mailbox,
            }),
        };
        let tap = SpectrumTap {
            consumer,
            sample_rate: sr_atomic,
        };
        Ok((handle, tap))
    }

    /// 切到这个 URL,从头播。已有曲目会被立刻打断。
    pub fn play(&self, url: MediaUrl) {
        self.send(AudioCommand::Play(url));
    }

    /// 暂停。
    pub fn pause(&self) {
        self.send(AudioCommand::Pause);
    }

    /// 从暂停恢复。
    pub fn resume(&self) {
        self.send(AudioCommand::Resume);
    }

    /// 停止当前曲目。
    pub fn stop(&self) {
        self.send(AudioCommand::Stop);
    }

    /// 跳到绝对位置(ms)。语义是 latest-wins:多次连按只生效最后一次,
    /// engine 主循环每 tick `take()` 一次实际打 demuxer。
    /// 部分容器 seek 不准是已知 trade-off。
    pub fn seek(&self, position_ms: u64) {
        *self.inner.seek_mailbox.lock() = Some(Duration::from_millis(position_ms));
    }

    /// 设置音量百分比(0..=100)。本地立刻更新 snapshot,免 UI 闪一帧旧值。
    pub fn set_volume(&self, pct: u8) {
        let clamped = pct.min(100);
        self.inner.snapshot.lock().volume_pct = clamped;
        self.send(AudioCommand::SetVolume(clamped));
    }

    /// UI tick 拉一次:engine 已经更新过的最新状态。
    pub fn snapshot(&self) -> AudioSnapshot {
        *self.inner.snapshot.lock()
    }

    fn send(&self, cmd: AudioCommand) {
        // engine 已退就忽略 —— UI 关闭路径上是合法的。
        let _ = self.inner.cmd_tx.send(cmd);
    }
}
