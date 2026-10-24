//! 建立指定设备的 CPAL 输出流，并保留建流时真正采用的格式。

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SizedSample, StreamError};
use mineral_common::revision::{AtomicRevision, Revision};

use super::devices;
use crate::decoder::PcmFormat;
use crate::engine::render::SharedPlayback;
use crate::{AudioOutput, Error, OutputTarget, Result};

/// 缓冲请求以约 50 ms 音频时长为目标；帧数随后取最近的 2 的幂。
const OUTPUT_BUFFERS_PER_SECOND: u32 = 20;

/// 一条已启动的系统流及其回调状态。
pub(super) struct DeviceStream {
    /// Owned system stream; dropping it retires this device callback.
    stream: cpal::Stream,

    /// 该流使用的设备与配置。
    pub(super) info: Arc<AudioOutput>,

    pub(super) format: PcmFormat,

    /// 系统异步报告的流错误。
    failed: Arc<AtomicBool>,

    /// 输出回调修订，用于停滞检测。
    callback_sequence: Arc<AtomicRevision>,
}

impl DeviceStream {
    /// 打开并启动目标流；输出管理器提交该代次前，新回调只输出静音。
    pub(super) fn open(
        target: OutputTarget,
        playback: Arc<SharedPlayback>,
        generation: Revision,
    ) -> Result<Self> {
        let device = devices::resolve(&target)?;
        let (device_id, device_name) = devices::describe(&device)?;
        let supported = device
            .default_output_config()
            .map_err(|source| Error::OutputConfig { source })?;
        let sample_format = supported.sample_format();
        let mut config = supported.config();
        config.buffer_size = cpal::BufferSize::Fixed(nearest_power_of_two(
            config.sample_rate / OUTPUT_BUFFERS_PER_SECOND,
        ));
        let channels = NonZeroU16::new(config.channels).ok_or(Error::ZeroChannels)?;
        let sample_rate = NonZeroU32::new(config.sample_rate).ok_or(Error::ZeroSampleRate)?;
        let format = PcmFormat {
            channels,
            sample_rate,
        };
        let failed = Arc::new(AtomicBool::new(false));
        let callback_sequence = Arc::new(AtomicRevision::default());
        let state = CallbackState {
            playback,
            format,
            generation,
            failed: Arc::clone(&failed),
            sequence: Arc::clone(&callback_sequence),
        };
        let stream = build_output_stream(&device, &config, sample_format, state)?;
        stream
            .play()
            .map_err(|source| Error::PlayStream { source })?;
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
            format,
            failed,
            callback_sequence,
        })
    }

    /// CPAL 是否已报告本流失效。
    pub(super) fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// 当前输出回调修订。
    pub(super) fn callback_sequence(&self) -> Revision {
        self.callback_sequence.load(Ordering::Relaxed)
    }

    /// 暂停并重启同一条系统流，保留播放队列。
    pub(super) fn restart(&self) -> Result<()> {
        self.stream
            .pause()
            .map_err(|source| Error::PauseStream { source })?;
        self.stream
            .play()
            .map_err(|source| Error::PlayStream { source })?;
        Ok(())
    }
}

/// One device callback's format, identity, and shared playback state.
struct CallbackState {
    /// Prepared PCM shared with worker threads.
    playback: Arc<SharedPlayback>,

    format: PcmFormat,

    generation: Revision,

    /// 异步错误标记。
    failed: Arc<AtomicBool>,

    /// 数据回调计数。
    sequence: Arc<AtomicRevision>,
}

/// 按系统样本类型建立输出回调。
fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    state: CallbackState,
) -> Result<cpal::Stream> {
    macro_rules! build {
        ($($format:ident, $sample:ty);+ $(;)?) => {
            match format {
                $(cpal::SampleFormat::$format => build_typed_output_stream::<$sample>(device, config, state),)+
                _ => Err(Error::UnsupportedSampleFormat { format }),
            }
        };
    }
    build!(F32, f32; F64, f64; I8, i8; I16, i16; I24, cpal::I24; I32, i32; I64, i64; U8, u8; U16, u16; U24, cpal::U24; U32, u32; U64, u64;)
}

/// Copy prepared PCM to the device; this callback does not read or decode media.
fn build_typed_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: CallbackState,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let CallbackState {
        playback,
        format,
        generation,
        failed,
        sequence,
    } = state;
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data, _| {
                sequence.next(Ordering::Relaxed);
                playback.render(data, format, generation);
            },
            move |error| match error {
                StreamError::BufferUnderrun => {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "audio output buffer underrun/overrun");
                }
                _ => {
                    failed.store(true, Ordering::Relaxed);
                    mineral_log::error!(target: "audio", error = mineral_log::chain(&error), "audio output stream error");
                }
            },
            None,
        )
        .map_err(|source| Error::BuildStream { source })
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
