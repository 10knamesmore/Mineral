//! 全曲振幅包络:整曲样本沿时间轴分桶取峰值,按全曲峰值归一后量化成定长 u8 序列。
//!
//! 与实时频谱(滑动窗 FFT)不同,这里是**离线一次性**计算:输入必须是可完整读取的
//! 本地文件(流播半截算不出正确的全曲形状)。归一化基准是全曲峰值而非绝对满刻度——
//! 波形要表达的是相对起伏,安静的曲目也该有可读的形状。

use std::num::{NonZeroU16, NonZeroUsize};

use color_eyre::eyre::eyre;
use mineral_model::Envelope;
use rodio::Source;

/// 包络定长点数:产出粒度,渲染端再按显示宽度二次重采样。
pub const ENVELOPE_POINT_COUNT: usize = 200;

/// 包络算法版本:分桶 / 归一 / 量化任一变更时 bump;读取方版本不符视同缺失、触发重算。
pub const ENVELOPE_VERSION: u16 = 1;

/// 分块粒度(mono 帧数):4 分钟 44.1kHz 约 10k 块,重采样到 200 点绰绰有余,
/// 又不必在内存里持整曲 PCM。
const CHUNK_FRAMES: usize = 1024;

/// 交错样本 → mono 帧平均 → 每 [`CHUNK_FRAMES`] 帧取一个峰值(|mono| 最大)。
///
/// 与实时 tap 同口径:**先并帧再取绝对值**,反相声道相互抵消。尾部不满一块的残帧
/// 也出一个峰值点;不满一帧的残样本丢弃。
///
/// # Params:
///   - `interleaved`: 交错样本(帧内按声道顺序)
///   - `channels`: 声道数(并帧口径)
///
/// # Return:
///   每块一个峰值;输入不足一帧时为空。
fn chunk_peaks(interleaved: impl Iterator<Item = f32>, channels: NonZeroU16) -> Vec<f32> {
    let n = channels.get();
    let mut peaks = Vec::new();
    let mut frame_sum = 0.0f32;
    let mut samples_in_frame: u16 = 0;
    let mut chunk_peak = 0.0f32;
    let mut frames_in_chunk: usize = 0;
    for s in interleaved {
        frame_sum += s;
        samples_in_frame = samples_in_frame.saturating_add(1);
        if samples_in_frame >= n {
            let mono = frame_sum / f32::from(n);
            chunk_peak = chunk_peak.max(mono.abs());
            frame_sum = 0.0;
            samples_in_frame = 0;
            frames_in_chunk += 1;
            if frames_in_chunk >= CHUNK_FRAMES {
                peaks.push(chunk_peak);
                chunk_peak = 0.0;
                frames_in_chunk = 0;
            }
        }
    }
    if frames_in_chunk > 0 {
        peaks.push(chunk_peak);
    }
    peaks
}

/// 峰值序列重采样到定长:缩小按桶取峰(不丢瞬态),放大按线性插值。
///
/// # Params:
///   - `peaks`: 输入峰值序列
///   - `out_len`: 输出点数
///
/// # Return:
///   定长 `out_len` 的序列;输入为空时为空。
#[allow(clippy::as_conversions)] // reason: 浮点插值坐标↔下标转换,越界已 clamp(同 spectrum 先例)
fn resample_peaks(peaks: &[f32], out_len: NonZeroUsize) -> Vec<f32> {
    let m = peaks.len();
    let n = out_len.get();
    if m == 0 {
        return Vec::new();
    }
    if m >= n {
        // 缩小:桶 i 覆盖 [i*m/n, (i+1)*m/n),取峰保瞬态。
        (0..n)
            .map(|i| {
                let lo = i * m / n;
                let hi = ((i + 1) * m / n).max(lo + 1).min(m);
                peaks
                    .get(lo..hi)
                    .unwrap_or_default()
                    .iter()
                    .copied()
                    .fold(0.0f32, f32::max)
            })
            .collect()
    } else {
        // 放大:归一化位置线性插值(m == 1 退化为常值)。
        let first = peaks.first().copied().unwrap_or_default();
        if m == 1 {
            return vec![first; n];
        }
        (0..n)
            .map(|i| {
                let t = (i as f32) / ((n - 1) as f32) * ((m - 1) as f32);
                let lo = (t.floor().max(0.0) as usize).min(m - 1);
                let frac = t - (lo as f32);
                let a = peaks.get(lo).copied().unwrap_or_default();
                let b = peaks.get(lo + 1).copied().unwrap_or(a);
                a + (b - a) * frac
            })
            .collect()
    }
}

/// 全曲峰值归一 + u8 量化:最大峰映射 255;全零(静音)全部落 0,不除零。
///
/// # Params:
///   - `peaks`: 峰值序列(非负)
///
/// # Return:
///   与输入等长的 0..=255 序列。
#[allow(clippy::as_conversions)] // reason: 浮点量化到 u8,值域已 clamp 进 0..=255
fn quantize(peaks: &[f32]) -> Vec<u8> {
    let max = peaks.iter().copied().fold(0.0f32, f32::max);
    if max <= 0.0 {
        return vec![0; peaks.len()];
    }
    peaks
        .iter()
        .map(|p| (p / max * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// 交错多声道样本流 → 定长振幅包络点。
///
/// # Params:
///   - `interleaved`: 交错样本(帧内按声道顺序)
///   - `channels`: 声道数(并帧口径)
///   - `point_count`: 输出点数
///
/// # Return:
///   `Some(points)`(len == `point_count`,0..=255,全曲峰值归一);
///   输入解不出任何完整帧时 `None`。
pub fn envelope_from_samples(
    interleaved: impl Iterator<Item = f32>,
    channels: NonZeroU16,
    point_count: NonZeroUsize,
) -> Option<Vec<u8>> {
    let peaks = chunk_peaks(interleaved, channels);
    if peaks.is_empty() {
        return None;
    }
    Some(quantize(&resample_peaks(&peaks, point_count)))
}

/// 离线解码整曲并计算包络。阻塞且 CPU 密集,调用方放 `spawn_blocking`。
///
/// 输入必须是**可完整读取**的本地音频文件(缓存 / 下载导出 / 本地曲库);
/// 流播半截的 capture 算不出正确的全曲形状,不要喂进来。
///
/// # Params:
///   - `path`: 本地音频文件路径
///
/// # Return:
///   定长 [`ENVELOPE_POINT_COUNT`] 点、版本 [`ENVELOPE_VERSION`] 的包络;
///   打开 / 解码失败或解不出任何完整帧时报错。
pub fn envelope_from_file(path: &std::path::Path) -> color_eyre::Result<Envelope> {
    let (reader, byte_len) = crate::engine::open_local(path)?;
    let decoder = crate::engine::build_decoder(reader, byte_len)?;
    let channels = decoder.channels();
    let point_count = NonZeroUsize::new(ENVELOPE_POINT_COUNT)
        .ok_or_else(|| eyre!("ENVELOPE_POINT_COUNT 不能为 0"))?;
    let points = envelope_from_samples(decoder, channels, point_count)
        .ok_or_else(|| eyre!("解不出任何完整帧: {}", path.display()))?;
    Ok(Envelope {
        points,
        version: ENVELOPE_VERSION,
    })
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroUsize};

    use proptest::prelude::{TestCaseError, prop, prop_assert_eq, proptest};
    use proptest::strategy::Strategy;

    use crate::envelope::envelope_from_samples;

    /// 测试用 `NonZeroU16` 构造(传 0 直接测试失败)。
    fn ch(n: u16) -> color_eyre::Result<NonZeroU16> {
        NonZeroU16::new(n).ok_or_else(|| color_eyre::eyre::eyre!("channels 不能为 0"))
    }

    /// 测试用 `NonZeroUsize` 构造(传 0 直接测试失败)。
    fn pts(n: usize) -> color_eyre::Result<NonZeroUsize> {
        NonZeroUsize::new(n).ok_or_else(|| color_eyre::eyre::eyre!("point_count 不能为 0"))
    }

    /// 恒定幅度全曲 → 所有点饱和 255:归一以全曲峰值为基准,与绝对音量无关。
    #[test]
    fn constant_amplitude_saturates_every_point() -> color_eyre::Result<()> {
        let samples = std::iter::repeat_n(0.25f32, 8192);
        let env = envelope_from_samples(samples, ch(1)?, pts(8)?)
            .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        assert_eq!(env, vec![255u8; 8]);
        Ok(())
    }

    /// 静音全曲 → 全 0:归一不得因除零产出 NaN / 垃圾值。
    #[test]
    fn silence_yields_all_zero() -> color_eyre::Result<()> {
        let samples = std::iter::repeat_n(0.0f32, 4096);
        let env = envelope_from_samples(samples, ch(1)?, pts(4)?)
            .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        assert_eq!(env, vec![0u8; 4]);
        Ok(())
    }

    /// 空样本 → `None`:「没有包络」用 Option 说话,不发明全零哨兵。
    #[test]
    fn empty_input_yields_none() -> color_eyre::Result<()> {
        let env = envelope_from_samples(std::iter::empty::<f32>(), ch(1)?, pts(8)?);
        assert_eq!(env, None);
        Ok(())
    }

    /// 线性渐强 → 点序列非降、末点 255:分桶与重采样都不得打乱时间轴次序。
    #[test]
    fn crescendo_is_non_decreasing() -> color_eyre::Result<()> {
        let samples = (0..=10_000u16).map(|i| f32::from(i) / 10_000.0);
        let env = envelope_from_samples(samples, ch(1)?, pts(5)?)
            .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        assert!(
            env.windows(2).all(|w| w.first() <= w.last()),
            "渐强的包络必须非降:{env:?}"
        );
        assert_eq!(env.last().copied(), Some(255));
        Ok(())
    }

    /// 整曲解码入口:真实 WAV(单声道线性渐强)解出定长 200 点 / 当前版本 /
    /// 非降形状且末点饱和——覆盖 open → decode → 分桶 → 量化的完整链路。
    #[test]
    fn envelope_from_file_decodes_real_wav() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("crescendo.wav");
        // 2 秒 8kHz 渐强:幅度 0 → 31_998 线性上升。
        let samples = (0..16_000i32)
            .map(|i| i16::try_from(i * 2))
            .collect::<Result<Vec<i16>, _>>()?;
        mineral_test::write_wav(
            &path, &samples, /*channels*/ 1, /*sample_rate*/ 8_000,
        )?;

        let env = crate::envelope::envelope_from_file(&path)?;
        assert_eq!(env.points.len(), crate::envelope::ENVELOPE_POINT_COUNT);
        assert_eq!(env.version, crate::envelope::ENVELOPE_VERSION);
        assert!(
            env.points.windows(2).all(|w| w.first() <= w.last()),
            "渐强 WAV 的包络必须非降"
        );
        assert_eq!(env.points.last().copied(), Some(255));
        Ok(())
    }

    /// 立体声先并帧再取峰:左右反相恰好抵消 → 全 0。
    /// (逐样本直接取 |s| 峰值会错误地得到全 255,此用例专门锁死并帧次序。)
    #[test]
    fn stereo_averages_frames_before_peak() -> color_eyre::Result<()> {
        let samples = (0..8192usize).map(|i| if i % 2 == 0 { 1.0f32 } else { -1.0f32 });
        let env = envelope_from_samples(samples, ch(2)?, pts(4)?)
            .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        assert_eq!(env, vec![0u8; 4]);
        Ok(())
    }

    proptest! {
        /// 输出恒为请求的定长:重采样(放大与缩小两个方向)不改变点数。
        #[test]
        fn output_length_matches_point_count(
            samples in prop::collection::vec(-1.0f32..1.0, 1..8192),
            point_count in (1usize..=64).prop_filter_map("非零点数", NonZeroUsize::new),
        ) {
            let env = envelope_from_samples(samples.into_iter(), NonZeroU16::MIN, point_count)
                .ok_or_else(|| TestCaseError::fail("非空输入必须产出包络"))?;
            prop_assert_eq!(env.len(), point_count.get());
        }

        /// 峰值归一:只要存在明确非零幅度,最大点必为 255。
        #[test]
        fn loudest_point_saturates(
            samples in prop::collection::vec(-1.0f32..1.0, 0..4096),
            point_count in (1usize..=64).prop_filter_map("非零点数", NonZeroUsize::new),
        ) {
            let mut samples = samples;
            samples.push(0.9);
            let env = envelope_from_samples(samples.into_iter(), NonZeroU16::MIN, point_count)
                .ok_or_else(|| TestCaseError::fail("非空输入必须产出包络"))?;
            prop_assert_eq!(env.iter().max().copied(), Some(255));
        }

        /// 整体缩放不变:全部样本同乘 0.5(f32 下精确)形状不变——包络表达相对起伏。
        #[test]
        fn envelope_is_scale_invariant(
            samples in prop::collection::vec(-1.0f32..1.0, 1..8192),
            point_count in (1usize..=64).prop_filter_map("非零点数", NonZeroUsize::new),
        ) {
            let original = envelope_from_samples(samples.iter().copied(), NonZeroU16::MIN, point_count);
            let halved =
                envelope_from_samples(samples.iter().map(|s| s * 0.5), NonZeroU16::MIN, point_count);
            prop_assert_eq!(original, halved);
        }
    }
}
