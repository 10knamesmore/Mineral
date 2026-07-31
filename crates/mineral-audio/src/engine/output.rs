//! 系统音频输出 stream 与停滞检测。

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use color_eyre::eyre::eyre;
use rodio::Source;
use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::cpal::{FromSample, Sample, SizedSample};

/// rodio 默认采用约 50ms 的输出 buffer；每秒 20 个 buffer 与其既有行为等价。
const OUTPUT_BUFFERS_PER_SECOND: u32 = 20;

/// active 播放时 output callback 连续停滞多久后尝试重启系统 stream。
const OUTPUT_STALL_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 1);

/// 可重启的系统音频输出。
///
/// Mineral 自己持有 [`cpal::Stream`]，从而能在系统音频 route 变化导致 callback 停止时
/// 原地重启 stream；rodio mixer 与 [`rodio::Player`] 不重建，decoder 队列和播放位置得以保留。
pub(super) struct Output {
    /// 系统输出 stream。
    stream: cpal::Stream,

    /// rodio 播放队列与控制面。
    player: rodio::Player,

    /// output data callback 的单调调用序号。
    callback_sequence: Arc<AtomicU64>,

    /// 系统 output callback 停滞检测器。
    watchdog: OutputWatchdog,
}

impl Output {
    /// 打开系统默认输出设备并启动 stream。
    ///
    /// # Params:
    ///   - `initial_gain`: rodio 线性初始增益。
    ///
    /// # Return:
    ///   已启动、可向 player append source 的输出。
    pub(super) fn open(initial_gain: f32) -> color_eyre::Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| eyre!("no default output device"))?;
        let supported = device
            .default_output_config()
            .map_err(|e| eyre!("default output config: {e}"))?;
        let sample_format = supported.sample_format();
        let mut config = supported.config();
        config.buffer_size = cpal::BufferSize::Fixed(nearest_power_of_two(
            config.sample_rate / OUTPUT_BUFFERS_PER_SECOND,
        ));

        let channels = NonZeroU16::new(config.channels)
            .ok_or_else(|| eyre!("default output config has zero channels"))?;
        let sample_rate = NonZeroU32::new(config.sample_rate)
            .ok_or_else(|| eyre!("default output config has zero sample rate"))?;
        let (mixer, source) = rodio::mixer::mixer(channels, sample_rate);
        let callback_sequence = Arc::new(AtomicU64::new(0));
        let stream = build_output_stream(
            &device,
            &config,
            sample_format,
            source,
            Arc::clone(&callback_sequence),
        )?;
        stream
            .play()
            .map_err(|e| eyre!("start output stream: {e}"))?;

        let player = rodio::Player::connect_new(&mixer);
        player.set_volume(initial_gain);
        Ok(Self {
            stream,
            player,
            callback_sequence,
            watchdog: OutputWatchdog::new(OUTPUT_STALL_TIMEOUT),
        })
    }

    /// 读取 rodio 播放控制面。
    pub(super) fn player(&self) -> &rodio::Player {
        &self.player
    }

    /// 读取 output data callback 的当前调用序号。
    pub(super) fn callback_sequence(&self) -> u64 {
        self.callback_sequence.load(Ordering::Relaxed)
    }

    /// 强制系统输出 stream 执行一次 stop/start，保留 rodio mixer 中的 source 队列。
    pub(super) fn restart_stream(&self) -> color_eyre::Result<()> {
        self.stream
            .pause()
            .map_err(|e| eyre!("pause stalled output stream: {e}"))?;
        self.stream
            .play()
            .map_err(|e| eyre!("restart stalled output stream: {e}"))
    }

    /// active 播放期间监测 output callback，停滞达到阈值后原地重启系统 stream。
    ///
    /// # Params:
    ///   - `playing`: 当前是否存在未暂停的播放曲目。
    ///   - `position_ms`: 当前 rodio 播放位置，写入恢复日志便于关联现场。
    pub(super) fn recover_if_stalled(&mut self, playing: bool, position_ms: u64) {
        let callback_sequence = self.callback_sequence();
        let activity = if playing {
            OutputActivity::Active
        } else {
            OutputActivity::Idle
        };
        if !self.watchdog.should_restart(WatchdogSample {
            observed_at: Instant::now(),
            activity,
            callback_sequence,
        }) {
            return;
        }

        mineral_log::warn!(
            target: "audio",
            position_ms,
            callback_sequence,
            "audio output callback stalled; restarting stream"
        );
        match self.restart_stream() {
            Ok(()) => {
                mineral_log::info!(
                    target: "audio",
                    position_ms,
                    callback_sequence,
                    "audio output stream restarted"
                );
            }
            Err(e) => {
                mineral_log::warn!(
                    target: "audio",
                    position_ms,
                    callback_sequence,
                    error = mineral_log::chain(&e),
                    "audio output stream restart failed"
                );
            }
        }
    }
}

/// 构建与设备 sample format 对应的系统输出 stream。
///
/// # Params:
///   - `device`: 系统音频输出设备。
///   - `config`: 设备 stream 配置。
///   - `sample_format`: callback 需要写入的 sample 格式。
///   - `samples`: rodio mixer 的 PCM 输出。
///   - `callback_sequence`: 每次 callback 更新的计数器。
///
/// # Return:
///   尚未启动的系统输出 stream。
fn build_output_stream<S>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    samples: S,
    callback_sequence: Arc<AtomicU64>,
) -> color_eyre::Result<cpal::Stream>
where
    S: Source + Send + 'static,
{
    macro_rules! build {
        ($($format:ident, $sample:ty);+ $(;)?) => {
            match sample_format {
                $(
                    cpal::SampleFormat::$format => build_typed_output_stream::<$sample, _>(
                        device,
                        config,
                        samples,
                        callback_sequence,
                    ),
                )+
                _ => Err(eyre!("unsupported output sample format: {sample_format:?}")),
            }
        };
    }

    build!(
        F32, f32;
        F64, f64;
        I8, i8;
        I16, i16;
        I24, cpal::I24;
        I32, i32;
        I64, i64;
        U8, u8;
        U16, u16;
        U24, cpal::U24;
        U32, u32;
        U64, u64;
    )
}

/// 构建一种具体 sample 类型的 output callback。
///
/// # Params:
///   - `device`: 系统音频输出设备。
///   - `config`: 设备 stream 配置。
///   - `samples`: rodio mixer 的 PCM 输出。
///   - `callback_sequence`: 每次 callback 更新的计数器。
///
/// # Return:
///   尚未启动的系统输出 stream。
fn build_typed_output_stream<T, S>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut samples: S,
    callback_sequence: Arc<AtomicU64>,
) -> color_eyre::Result<cpal::Stream>
where
    T: SizedSample + FromSample<rodio::Sample>,
    S: Source + Send + 'static,
{
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data, _| {
                callback_sequence.fetch_add(1, Ordering::Relaxed);
                data.iter_mut().for_each(|sample| {
                    *sample = samples
                        .next()
                        .map(Sample::from_sample)
                        .unwrap_or(T::EQUILIBRIUM);
                });
            },
            |error| log_stream_error(&error),
            /*timeout*/ None,
        )
        .map_err(|e| eyre!("build output stream: {e}"))
}

/// 把 cpal 的异步 stream error 接入 daemon 日志。
///
/// # Params:
///   - `error`: cpal 上报的 stream error。
fn log_stream_error(error: &cpal::StreamError) {
    let error = eyre!("cpal output stream: {error}");
    mineral_log::error!(
        target: "audio",
        error = mineral_log::chain(&error),
        "audio output stream error"
    );
}

/// 返回最接近 `value` 的 2 的幂，供固定 output buffer 使用。
///
/// # Params:
///   - `value`: 目标 frame 数。
///
/// # Return:
///   至少为 1 的 2 的幂。
fn nearest_power_of_two(value: u32) -> u32 {
    if value <= 1 {
        return 1;
    }
    let Some(next) = value.checked_next_power_of_two() else {
        return u32::MAX / 2 + 1;
    };
    let previous = next >> 1;
    if value - previous <= next - value {
        previous
    } else {
        next
    }
}

/// 当前 rodio 队列是否应该持续收到 output callback。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OutputActivity {
    /// 当前有曲目且未暂停。
    Active,

    /// 暂停、停止或队列为空。
    Idle,
}

/// 一次 output callback 健康检查的输入。
#[derive(Clone, Copy, Debug)]
pub(super) struct WatchdogSample {
    /// 本次检查的单调时间。
    pub(super) observed_at: Instant,

    /// 当前播放活动状态。
    pub(super) activity: OutputActivity,

    /// output data callback 的当前调用序号。
    pub(super) callback_sequence: u64,
}

/// 通过 callback 序号判断系统输出 stream 是否停止回调。
pub(super) struct OutputWatchdog {
    /// active 状态下允许 callback 不变的最长时间。
    timeout: Duration,

    /// 上次观测到的 callback 序号。
    last_callback_sequence: Option<u64>,

    /// 当前连续无 callback 的起点；idle 或序号推进时清空。
    stalled_since: Option<Instant>,
}

impl OutputWatchdog {
    /// 构造停滞检测器。
    ///
    /// # Params:
    ///   - `timeout`: active 状态下 callback 连续不变多久后触发恢复。
    pub(super) fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            last_callback_sequence: None,
            stalled_since: None,
        }
    }

    /// 记录一次观测并判断当前是否应该重启 stream。
    ///
    /// 触发后把起点推进到本次观测时间，因此持续故障最多每个 `timeout` 重试一次。
    ///
    /// # Params:
    ///   - `sample`: 当前活动状态、callback 序号与单调时间。
    ///
    /// # Return:
    ///   仅当 active callback 连续停滞达到阈值时为 `true`。
    pub(super) fn should_restart(&mut self, sample: WatchdogSample) -> bool {
        let callback_advanced = self.last_callback_sequence != Some(sample.callback_sequence);
        self.last_callback_sequence = Some(sample.callback_sequence);

        if sample.activity == OutputActivity::Idle || callback_advanced {
            self.stalled_since = None;
            return false;
        }

        let Some(stalled_since) = self.stalled_since else {
            self.stalled_since = Some(sample.observed_at);
            return false;
        };
        if sample.observed_at.saturating_duration_since(stalled_since) < self.timeout {
            return false;
        }

        self.stalled_since = Some(sample.observed_at);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{OutputActivity, OutputWatchdog, WatchdogSample};

    #[test]
    fn watchdog_restarts_only_an_active_stream_without_callbacks() {
        let timeout = Duration::from_secs(/*secs*/ 1);
        let started_at = Instant::now();
        let mut watchdog = OutputWatchdog::new(timeout);

        assert!(!watchdog.should_restart(WatchdogSample {
            observed_at: started_at,
            activity: OutputActivity::Idle,
            callback_sequence: 7,
        }));
        assert!(!watchdog.should_restart(WatchdogSample {
            observed_at: started_at + timeout,
            activity: OutputActivity::Active,
            callback_sequence: 7,
        }));
        assert!(!watchdog.should_restart(WatchdogSample {
            observed_at: started_at + timeout + timeout - Duration::from_millis(/*millis*/ 1),
            activity: OutputActivity::Active,
            callback_sequence: 7,
        }));
        assert!(watchdog.should_restart(WatchdogSample {
            observed_at: started_at + timeout + timeout,
            activity: OutputActivity::Active,
            callback_sequence: 7,
        }));
        assert!(!watchdog.should_restart(WatchdogSample {
            observed_at: started_at + timeout + timeout + timeout,
            activity: OutputActivity::Active,
            callback_sequence: 8,
        }));
    }
}
