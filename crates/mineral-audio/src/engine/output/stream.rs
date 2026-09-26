//! 建立指定设备的 CPAL 输出流，并保留建流时真正采用的格式。

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use color_eyre::eyre::eyre;
use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, StreamTrait};
use rodio::cpal::{FromSample, Sample, SizedSample};

use super::devices;
use super::source::{QueueHandoff, take_samples};
use crate::{AudioOutput, OutputTarget};

/// 缓冲请求以约 50 ms 音频时长为目标；帧数随后取最近的 2 的幂。
const OUTPUT_BUFFERS_PER_SECOND: u32 = 20;

/// 一条已启动的系统流及其回调状态。
pub(super) struct DeviceStream {
    /// 销毁时回调归还队列，供下一条流继续读取。
    stream: cpal::Stream,

    /// 该流使用的设备与配置。
    pub(super) info: Arc<AudioOutput>,

    /// 系统异步报告的流错误。
    failed: Arc<AtomicBool>,

    /// 输出回调次数，用于停滞检测。
    callback_sequence: Arc<AtomicU64>,
}

impl DeviceStream {
    /// 先打开并启动目标流；旧流仍持有队列时，新流只输出静音。
    pub(super) fn open(target: OutputTarget, handoff: QueueHandoff) -> color_eyre::Result<Self> {
        let device = devices::resolve(&target)?;
        let (device_id, device_name) = devices::describe(&device)?;
        let supported = device.default_output_config()?;
        let sample_format = supported.sample_format();
        let mut config = supported.config();
        config.buffer_size = cpal::BufferSize::Fixed(nearest_power_of_two(
            config.sample_rate / OUTPUT_BUFFERS_PER_SECOND,
        ));
        let channels = NonZeroU16::new(config.channels)
            .ok_or_else(|| eyre!("output config has zero channels"))?;
        let sample_rate = NonZeroU32::new(config.sample_rate)
            .ok_or_else(|| eyre!("output config has zero sample rate"))?;
        let failed = Arc::new(AtomicBool::new(false));
        let callback_sequence = Arc::new(AtomicU64::new(0));
        let state = CallbackState {
            handoff,
            channels,
            sample_rate,
            failed: Arc::clone(&failed),
            sequence: Arc::clone(&callback_sequence),
        };
        let stream = build_output_stream(&device, &config, sample_format, state)?;
        stream.play()?;
        let info = AudioOutput {
            target,
            device_id,
            device_name,
            sample_rate_hz: config.sample_rate,
            channels: config.channels,
            sample_format: sample_format.to_string(),
        };
        Ok(Self {
            stream,
            info: Arc::new(info),
            failed,
            callback_sequence,
        })
    }

    /// CPAL 是否已报告本流失效。
    pub(super) fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// 当前输出回调次数。
    pub(super) fn callback_sequence(&self) -> u64 {
        self.callback_sequence.load(Ordering::Relaxed)
    }

    /// 暂停并重启同一条系统流，保留播放队列。
    pub(super) fn restart(&self) -> color_eyre::Result<()> {
        self.stream.pause()?;
        self.stream.play()?;
        Ok(())
    }
}

/// 转移给一个系统回调的队列入口与运行状态。
struct CallbackState {
    /// 旧回调释放后可接管的播放队列。
    handoff: QueueHandoff,

    /// 新流的声道数量。
    channels: rodio::ChannelCount,

    /// 新流的采样率。
    sample_rate: rodio::SampleRate,

    /// 异步错误标记。
    failed: Arc<AtomicBool>,

    /// 数据回调计数。
    sequence: Arc<AtomicU64>,
}

/// 按系统样本类型建立输出回调。
fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    state: CallbackState,
) -> color_eyre::Result<cpal::Stream> {
    macro_rules! build {
        ($($format:ident, $sample:ty);+ $(;)?) => {
            match format {
                $(cpal::SampleFormat::$format => build_typed_output_stream::<$sample>(device, config, state),)+
                _ => Err(eyre!("unsupported output sample format: {format:?}")),
            }
        };
    }
    build!(F32, f32; F64, f64; I8, i8; I16, i16; I24, cpal::I24; I32, i32; I64, i64; U8, u8; U16, u16; U24, cpal::U24; U32, u32; U64, u64;)
}

/// 通过移交锁接管队列，再由 rodio 转换声道和采样率，逐个填入输出采样。
fn build_typed_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: CallbackState,
) -> color_eyre::Result<cpal::Stream>
where
    T: SizedSample + FromSample<rodio::Sample>,
{
    let mut samples = None;
    let CallbackState {
        handoff,
        channels,
        sample_rate,
        failed,
        sequence,
    } = state;
    Ok(device.build_output_stream::<T, _, _>(
        config,
        move |data, _| {
            sequence.fetch_add(1, Ordering::Relaxed);
            if samples.is_none() {
                samples = take_samples(&handoff, channels, sample_rate);
            }
            for sample in data {
                *sample = samples
                    .as_mut()
                    .and_then(Iterator::next)
                    .map(Sample::from_sample)
                    .unwrap_or(T::EQUILIBRIUM);
            }
        },
        move |error| {
            failed.store(true, Ordering::Relaxed);
            mineral_log::error!(target: "audio", error = %error, "audio output stream error");
        },
        None,
    )?)
}

/// 返回最接近目标帧数的 2 的幂，最小为 1。
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
