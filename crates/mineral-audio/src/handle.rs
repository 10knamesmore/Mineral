//! UI 持有的 audio handle:线程安全、可 clone,所有方法都是非阻塞。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
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

/// 引擎启动时的音频后端选择。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioMode {
    /// 自动:尝试打开默认输出设备,失败则降级到 null(无声但引擎仍活)。
    #[default]
    Auto,

    /// 强制 null:不碰设备,直接空跑。用于无音频环境 / e2e 测试确定性复现降级。
    ForceNull,
}

/// 共享内部状态:命令通道 + snapshot + seek mailbox(latest-wins)。
struct Inner {
    /// 命令通道发送端,handle 把 [`AudioCommand`] 推给 engine 线程。
    cmd_tx: mpsc::Sender<AudioCommand>,

    /// engine 周期性写入、UI tick 读取的最新播放状态。
    snapshot: Arc<Mutex<AudioSnapshot>>,

    /// 最新待执行的 seek 目标位置;engine 每 tick `take()` 一次实际打 demuxer,长按 ←/→ 时只生效最后一次。
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
    /// 共享内部状态(命令通道 / snapshot / seek mailbox)。
    inner: Arc<Inner>,
}

impl AudioHandle {
    /// 启动 engine 线程并返回 (handle, spectrum tap)。
    ///
    /// # Params:
    ///   - `mode`: [`AudioMode::Auto`] 拿不到设备时降级 null;[`AudioMode::ForceNull`] 直接空跑。
    ///
    /// # Return:
    ///   `Err` 仅在引擎线程 spawn / runtime 构建等**真错**时返回;无音频设备**不**算错(降级)。
    pub fn spawn(mode: AudioMode) -> color_eyre::Result<(Self, SpectrumTap)> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<AudioCommand>();
        let snapshot = Arc::new(Mutex::new(AudioSnapshot {
            volume_pct: 100,
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
                    mode,
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

    /// 内部统一的发送入口:engine 已退时静默忽略(UI 关闭路径合法)。
    fn send(&self, cmd: AudioCommand) {
        // engine 已退就忽略 —— UI 关闭路径上是合法的。
        let _ = self.inner.cmd_tx.send(cmd);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::handle::AudioMode;
    use crate::snapshot::AudioBackend;

    use super::AudioHandle;

    /// `ForceNull` 起得来、snapshot 标 `Null`,且无 sink 也接受命令、引擎线程不死。
    ///
    /// 无 env、无真音频设备,确定性复现降级路径。
    #[test]
    fn force_null_spawns_in_null_mode_and_accepts_commands() -> color_eyre::Result<()> {
        let (handle, _tap) = AudioHandle::spawn(AudioMode::ForceNull)?;
        assert_eq!(
            handle.snapshot().backend,
            AudioBackend::Null,
            "ForceNull 应让 snapshot.backend == Null"
        );

        // 无 sink 也得吃命令、不 panic;set_volume 由 handle 直接写 snapshot。
        handle.set_volume(50);
        handle.pause();
        handle.resume();
        handle.stop();
        assert_eq!(
            handle.snapshot().volume_pct,
            50,
            "set_volume 应即时反映在 snapshot"
        );

        // 引擎线程仍活:发命令后短暂等待,snapshot 仍可读(锁没被毒化)。
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(handle.snapshot().backend, AudioBackend::Null);
        Ok(())
    }
}
