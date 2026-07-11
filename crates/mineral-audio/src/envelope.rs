//! 全曲响度包络:整曲样本 mono 并帧 → BS.1770 K-weighting 预滤波 → 100ms 块均方 →
//! 400ms 滑窗 momentary 响度,按全曲最大响度归一后量化成定长 u8 序列。
//!
//! 统计量演进:峰值(v1)在响度战争压限的音乐上处处顶满,包络退化成全满砖墙;
//! 平直 RMS(v2)保住了能量形状,但鼓 / sub-bass 能量大而听感不成比例,条高仍虚高。
//! v3 换 ITU-R BS.1770 的 K-weighting(高频搁架 + 低频高通)加 400ms momentary
//! 时间窗,让条高贴近听感响度。刻意偏离标准 LUFS 的部分:mono 下混后再滤波(与
//! 实时 tap 同口径,标准是逐声道滤波加权求和)、无 gating(安静段正是要画的信息)、
//! 按全曲最大归一(表达相对起伏,安静曲目也要有可读形状)——只借频率计权与时间窗,
//! 不产出绝对响度值。
//!
//! 与实时频谱(滑动窗 FFT)不同,这里是**离线一次性**计算:输入必须是可完整读取的
//! 本地文件(流播半截算不出正确的全曲形状)。

use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};

use color_eyre::eyre::eyre;
use mineral_model::Envelope;
use rodio::Source;

/// 包络算法版本:滤波 / 分块统计 / 归一 / 量化的**结构**变更时 bump;读取方版本
/// 不符视同缺失、触发重算。v3 = RMS 换 K-weighting + momentary 时窗(块粒度也从
/// 固定帧数改为固定时长,消除时间粒度随采样率漂移)。
///
/// 注意:[`EnvelopeParams`] 的数值变更**不**反映到版本里——改参数只影响之后计算
/// 的包络,已落库的不自动重算。
pub const ENVELOPE_VERSION: u16 = 3;

/// 包络计算参数(配置 `audio.envelope` 的切片,daemon 启动时派生)。
///
/// 默认值(在 `default.lua`)即 BS.1770 规范参数。私有字段 + builder 构造 +
/// getter 读取(对外配置 struct 约定)。
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct EnvelopeParams {
    /// 包络定长点数(产出粒度,渲染端再按显示宽度二次重采样)。
    point_count: std::num::NonZeroUsize,

    /// 响度块时长(毫秒,块内取均方)。
    block_ms: NonZeroU32,

    /// momentary 滑窗时长(毫秒;规范口径 = 4 × 块时长,75% 重叠)。
    window_ms: NonZeroU32,

    /// K-weighting 高频搁架级参数。
    shelf: ShelfParams,

    /// K-weighting RLB 高通级参数。
    highpass: HighpassParams,
}

/// 高频搁架(头部声学)滤波的模拟原型参数;按采样率经双线性变换推导数字系数。
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct ShelfParams {
    /// 转折频率(Hz)。
    f0_hz: f64,

    /// 搁架增益(dB)。
    gain_db: f64,

    /// 品质因数。
    q: f64,

    /// 过渡带增益分配指数(`Vb = Vh^band_exponent`);与其余参数同为参考实现
    /// 从规范系数表反推的模拟原型参数。
    band_exponent: f64,
}

/// RLB 高通(人耳低频不敏感)滤波的模拟原型参数;按采样率经双线性变换推导数字系数。
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct HighpassParams {
    /// 转折频率(Hz)。
    f0_hz: f64,

    /// 品质因数。
    q: f64,
}

/// 二阶 IIR 滤波节(转置直接 II 型)。
///
/// 系数与状态都用 f64:38Hz 高通在高采样率下极点紧贴单位圆,f32 精度会让
/// 状态漂移、低频衰减失真。
struct Biquad {
    /// 分子系数 z^0。
    b0: f64,
    /// 分子系数 z^-1。
    b1: f64,
    /// 分子系数 z^-2。
    b2: f64,
    /// 分母系数 z^-1(a0 已归一进其余系数)。
    a1: f64,
    /// 分母系数 z^-2。
    a2: f64,
    /// 延迟状态 1。
    z1: f64,
    /// 延迟状态 2。
    z2: f64,
}

impl Biquad {
    /// 处理一个样本,推进内部状态。
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// BS.1770 K-weighting 预滤波:高频搁架(头部声学,~2kHz 以上 +4dB)+ RLB 高通
/// (38Hz,人耳低频不敏感)两级级联。
///
/// 规范只给出 48kHz 的系数表;任意采样率按模拟原型参数(f0 / G / Q)经双线性变换
/// 现场推导。参数与推导式对照 libebur128,48kHz 下与规范系数表逐位一致(见测试)。
struct KWeighting {
    /// 高频搁架级。
    shelf: Biquad,
    /// RLB 高通级。
    highpass: Biquad,
}

impl KWeighting {
    /// 按采样率与模拟原型参数推导两级系数。
    fn new(sample_rate: f64, shelf: &ShelfParams, highpass: &HighpassParams) -> Self {
        let q = *shelf.q();
        let k = (std::f64::consts::PI * shelf.f0_hz() / sample_rate).tan();
        let vh = 10.0f64.powf(shelf.gain_db() / 20.0);
        let vb = vh.powf(*shelf.band_exponent());
        let a0 = 1.0 + k / q + k * k;
        let shelf = Biquad {
            b0: (vh + vb * k / q + k * k) / a0,
            b1: 2.0 * (k * k - vh) / a0,
            b2: (vh - vb * k / q + k * k) / a0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            z1: 0.0,
            z2: 0.0,
        };
        let q = *highpass.q();
        let k = (std::f64::consts::PI * highpass.f0_hz() / sample_rate).tan();
        let a0 = 1.0 + k / q + k * k;
        // 分子固定 [1, -2, 1] 不按 a0 归一——规范原文如此,通带增益仍 ≈ 0dB。
        let highpass = Biquad {
            b0: 1.0,
            b1: -2.0,
            b2: 1.0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            z1: 0.0,
            z2: 0.0,
        };
        Self { shelf, highpass }
    }

    /// 处理一个 mono 样本(两级级联)。
    fn process(&mut self, x: f64) -> f64 {
        self.highpass.process(self.shelf.process(x))
    }
}

/// 交错样本 → mono 帧平均 → K-weighting → 定长块均方 → 尾对齐滑窗开方,
/// 得每块一个的 momentary 响度序列。
///
/// 与实时 tap 同口径:**先并帧再滤波统计**,反相声道相互抵消。块粒度按采样率折算
/// 固定时长(固定帧数会让时间粒度随采样率漂移)。尾部不满一块的残帧也出一块
/// (按实际帧数取均值);不满一帧的残样本丢弃;开头不足一个滑窗处按实际块数取窗。
///
/// # Params:
///   - `interleaved`: 交错样本(帧内按声道顺序)
///   - `channels`: 声道数(并帧口径)
///   - `sample_rate`: 采样率(Hz,定块时长)
///   - `params`: 计算参数(块 / 滑窗时长与滤波原型)
///
/// # Return:
///   每块一个 momentary 响度;输入不足一帧时为空。
fn momentary_levels(
    interleaved: impl Iterator<Item = f32>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    params: &EnvelopeParams,
) -> Vec<f64> {
    let n = channels.get();
    let frames_per_block = u32::try_from(
        (u64::from(sample_rate.get()) * u64::from(params.block_ms().get()) / 1_000).max(1),
    )
    .unwrap_or(u32::MAX);
    let window_blocks = usize::try_from(params.window_ms().get() / params.block_ms().get())
        .unwrap_or(1)
        .max(1);
    let mut filter = KWeighting::new(
        f64::from(sample_rate.get()),
        params.shelf(),
        params.highpass(),
    );
    let mut block_mean_squares = Vec::<f64>::new();
    let mut frame_sum = 0.0f32;
    let mut samples_in_frame: u16 = 0;
    let mut square_sum = 0.0f64;
    let mut frames_in_block: u32 = 0;
    for s in interleaved {
        frame_sum += s;
        samples_in_frame = samples_in_frame.saturating_add(1);
        if samples_in_frame >= n {
            let mono = frame_sum / f32::from(n);
            let weighted = filter.process(f64::from(mono));
            square_sum += weighted * weighted;
            frame_sum = 0.0;
            samples_in_frame = 0;
            frames_in_block = frames_in_block.saturating_add(1);
            if frames_in_block >= frames_per_block {
                block_mean_squares.push(square_sum / f64::from(frames_in_block));
                square_sum = 0.0;
                frames_in_block = 0;
            }
        }
    }
    if frames_in_block > 0 {
        block_mean_squares.push(square_sum / f64::from(frames_in_block));
    }
    (0..block_mean_squares.len())
        .map(|i| {
            let lo = i.saturating_sub(window_blocks - 1);
            let window = block_mean_squares.get(lo..=i).unwrap_or_default();
            let count = u32::try_from(window.len()).unwrap_or(1).max(1);
            (window.iter().sum::<f64>() / f64::from(count)).sqrt()
        })
        .collect()
}

/// 响度序列重采样到定长:缩小按桶取最大值(保住最响块),放大按线性插值。
///
/// # Params:
///   - `levels`: 输入响度序列
///   - `out_len`: 输出点数
///
/// # Return:
///   定长 `out_len` 的序列;输入为空时为空。
#[allow(clippy::as_conversions)] // reason: 浮点插值坐标↔下标转换,越界已 clamp(同 spectrum 先例)
fn resample_levels(levels: &[f64], out_len: NonZeroUsize) -> Vec<f64> {
    let m = levels.len();
    let n = out_len.get();
    if m == 0 {
        return Vec::new();
    }
    if m >= n {
        // 缩小:桶 i 覆盖 [i*m/n, (i+1)*m/n),取最大值保住最响块。
        (0..n)
            .map(|i| {
                let lo = i * m / n;
                let hi = ((i + 1) * m / n).max(lo + 1).min(m);
                levels
                    .get(lo..hi)
                    .unwrap_or_default()
                    .iter()
                    .copied()
                    .fold(0.0f64, f64::max)
            })
            .collect()
    } else {
        // 放大:归一化位置线性插值(m == 1 退化为常值)。
        let first = levels.first().copied().unwrap_or_default();
        if m == 1 {
            return vec![first; n];
        }
        (0..n)
            .map(|i| {
                let t = (i as f64) / ((n - 1) as f64) * ((m - 1) as f64);
                let lo = (t.floor().max(0.0) as usize).min(m - 1);
                let frac = t - (lo as f64);
                let a = levels.get(lo).copied().unwrap_or_default();
                let b = levels.get(lo + 1).copied().unwrap_or(a);
                a + (b - a) * frac
            })
            .collect()
    }
}

/// 全曲最大响度归一 + u8 量化:最响块映射 255;全零(静音)全部落 0,不除零。
///
/// # Params:
///   - `levels`: 响度序列(非负)
///
/// # Return:
///   与输入等长的 0..=255 序列。
#[allow(clippy::as_conversions)] // reason: 浮点量化到 u8,值域已 clamp 进 0..=255
fn quantize(levels: &[f64]) -> Vec<u8> {
    let max = levels.iter().copied().fold(0.0f64, f64::max);
    if max <= 0.0 {
        return vec![0; levels.len()];
    }
    levels
        .iter()
        .map(|level| (level / max * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// 交错多声道样本流 → 定长响度包络点。
///
/// # Params:
///   - `interleaved`: 交错样本(帧内按声道顺序)
///   - `channels`: 声道数(并帧口径)
///   - `sample_rate`: 采样率(Hz,K-weighting 系数与块时长都依赖它)
///   - `params`: 计算参数(配置 `audio.envelope` 切片)
///
/// # Return:
///   `Some(points)`(len == `params.point_count`,0..=255,全曲最大 momentary
///   响度归一);输入解不出任何完整帧时 `None`。
pub fn envelope_from_samples(
    interleaved: impl Iterator<Item = f32>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    params: &EnvelopeParams,
) -> Option<Vec<u8>> {
    let levels = momentary_levels(interleaved, channels, sample_rate, params);
    if levels.is_empty() {
        return None;
    }
    Some(quantize(&resample_levels(&levels, *params.point_count())))
}

/// 离线解码整曲并计算包络。阻塞且 CPU 密集,调用方放 `spawn_blocking`。
///
/// 输入必须是**可完整读取**的本地音频文件(缓存 / 下载导出 / 本地曲库);
/// 流播半截的 capture 算不出正确的全曲形状,不要喂进来。
///
/// # Params:
///   - `path`: 本地音频文件路径
///   - `params`: 计算参数(配置 `audio.envelope` 切片)
///
/// # Return:
///   定长 `params.point_count` 点、版本 [`ENVELOPE_VERSION`] 的包络;
///   打开 / 解码失败或解不出任何完整帧时报错。
pub fn envelope_from_file(
    path: &std::path::Path,
    params: &EnvelopeParams,
) -> color_eyre::Result<Envelope> {
    let (reader, byte_len) = crate::engine::open_local(path)?;
    let decoder = crate::engine::build_decoder(reader, byte_len)?;
    let channels = decoder.channels();
    let sample_rate = decoder.sample_rate();
    let points = envelope_from_samples(decoder, channels, sample_rate, params)
        .ok_or_else(|| eyre!("解不出任何完整帧: {}", path.display()))?;
    Ok(Envelope {
        points,
        version: ENVELOPE_VERSION,
    })
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};

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

    /// 测试用 `NonZeroU32` 采样率构造(传 0 直接测试失败)。
    fn rate(hz: u32) -> color_eyre::Result<NonZeroU32> {
        NonZeroU32::new(hz).ok_or_else(|| color_eyre::eyre::eyre!("sample_rate 不能为 0"))
    }

    /// 测试用计算参数(default.lua 同款默认值,点数可指定)。
    fn params(point_count: NonZeroUsize) -> super::EnvelopeParams {
        super::EnvelopeParams::builder()
            .point_count(point_count)
            .block_ms(NonZeroU32::new(100).unwrap_or(NonZeroU32::MIN))
            .window_ms(NonZeroU32::new(400).unwrap_or(NonZeroU32::MIN))
            .shelf(shelf_params())
            .highpass(highpass_params())
            .build()
    }

    /// BS.1770 高频搁架模拟原型参数(default.lua 同款默认值)。
    fn shelf_params() -> super::ShelfParams {
        super::ShelfParams::builder()
            .f0_hz(1_681.974_450_955_533)
            .gain_db(3.999_843_853_973_347)
            .q(0.707_175_236_955_419_6)
            .band_exponent(0.499_666_774_154_541_6)
            .build()
    }

    /// BS.1770 RLB 高通模拟原型参数(default.lua 同款默认值)。
    fn highpass_params() -> super::HighpassParams {
        super::HighpassParams::builder()
            .f0_hz(38.135_470_876_024_44)
            .q(0.500_327_037_323_877_3)
            .build()
    }

    /// 定幅正弦音。相位逐样本递增累加,不做下标 → 浮点转换(`as` 全禁)。
    fn tone(freq_hz: f32, amplitude: f32, sample_rate: u16, len: usize) -> Vec<f32> {
        let step = std::f32::consts::TAU * freq_hz / f32::from(sample_rate);
        let mut phase = 0.0f32;
        (0..len)
            .map(|_| {
                let s = amplitude * phase.sin();
                phase += step;
                s
            })
            .collect()
    }

    /// 恒定幅度正弦全曲 → 形状平坦近满:归一以全曲最大响度为基准,与绝对音量无关。
    /// (首块含滤波器 warm-up,允许小幅偏差,不逐点断言 255。)
    #[test]
    fn constant_tone_saturates_every_point() -> color_eyre::Result<()> {
        let samples = tone(1_000.0, 0.25, 8_000, 16_000);
        let env =
            envelope_from_samples(samples.into_iter(), ch(1)?, rate(8_000)?, &params(pts(8)?))
                .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        assert_eq!(env.iter().max().copied(), Some(255));
        assert!(
            env.iter().all(|&p| p >= 230),
            "恒幅正弦的包络应平坦近满:{env:?}"
        );
        Ok(())
    }

    /// 静音全曲 → 全 0:归一不得因除零产出 NaN / 垃圾值。
    #[test]
    fn silence_yields_all_zero() -> color_eyre::Result<()> {
        let samples = std::iter::repeat_n(0.0f32, 4096);
        let env = envelope_from_samples(samples, ch(1)?, rate(8_000)?, &params(pts(4)?))
            .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        assert_eq!(env, vec![0u8; 4]);
        Ok(())
    }

    /// 空样本 → `None`:「没有包络」用 Option 说话,不发明全零哨兵。
    #[test]
    fn empty_input_yields_none() -> color_eyre::Result<()> {
        let env = envelope_from_samples(
            std::iter::empty::<f32>(),
            ch(1)?,
            rate(8_000)?,
            &params(pts(8)?),
        );
        assert_eq!(env, None);
        Ok(())
    }

    /// 幅度线性渐强的正弦 → 点序列非降、末点 255:分块 / 滑窗 / 重采样都不得打乱
    /// 时间轴次序。(载波用正弦而非裸 ramp:ramp 是直流内容,会被 K 计权高通滤掉。)
    #[test]
    fn crescendo_is_non_decreasing() -> color_eyre::Result<()> {
        let n = 16_000u16;
        let step = std::f32::consts::TAU * 440.0 / 8_000.0;
        let amp_step = 1.0 / f32::from(n);
        let mut phase = 0.0f32;
        let mut amp = 0.0f32;
        let samples = (0..n).map(move |_| {
            let s = amp * phase.sin();
            phase += step;
            amp += amp_step;
            s
        });
        let env = envelope_from_samples(samples, ch(1)?, rate(8_000)?, &params(pts(5)?))
            .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        assert!(
            env.windows(2).all(|w| w.first() <= w.last()),
            "渐强的包络必须非降:{env:?}"
        );
        assert_eq!(env.last().copied(), Some(255));
        Ok(())
    }

    /// 响度形状而非峰值形状:「安静但带零星同幅尖峰」段必须显著低于持续大声段。
    /// 峰值统计在响度战争压限的音乐上处处顶满 → 波形全满砖墙;包络要表达的是
    /// verse / 副歌的能量差。安静段放前面:momentary 是尾对齐 400ms 滑窗,
    /// 大声段放前会把窗尾拖进后续桶,污染断言。
    #[test]
    fn sparse_transients_do_not_saturate_quiet_sections() -> color_eyre::Result<()> {
        let quiet_with_spikes = tone(500.0, 0.05, 8_000, 8_000)
            .into_iter()
            .enumerate()
            .map(|(i, s)| if i % 128 == 0 { 0.8f32 } else { s });
        let loud = tone(500.0, 0.8, 8_000, 8_000);
        let env = envelope_from_samples(
            quiet_with_spikes.chain(loud),
            ch(1)?,
            rate(8_000)?,
            &params(pts(4)?),
        )
        .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        let quiet_max = env
            .get(..2)
            .unwrap_or_default()
            .iter()
            .copied()
            .max()
            .unwrap_or(0);
        assert!(quiet_max < 128, "安静段的零星尖峰不得把包络顶满:{env:?}");
        assert_eq!(
            env.get(2..).unwrap_or_default().iter().max().copied(),
            Some(255),
            "持续大声段应满格:{env:?}"
        );
        Ok(())
    }

    /// K 计权的低频衰减:同幅度 25Hz(sub-bass)段的包络必须显著低于 500Hz(中频)段
    /// ——鼓 / sub-bass 能量大但听感不成比例,这正是 v3 换 K-weighting 的动机。
    /// 低频段放前面:momentary 尾对齐滑窗只向前看,前段不受后段污染。
    #[test]
    fn sub_bass_reads_far_quieter_than_midrange() -> color_eyre::Result<()> {
        let bass = tone(25.0, 0.5, 8_000, 16_000);
        let mid = tone(500.0, 0.5, 8_000, 16_000);
        let env = envelope_from_samples(
            bass.into_iter().chain(mid),
            ch(1)?,
            rate(8_000)?,
            &params(pts(4)?),
        )
        .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        let bass_max = env
            .get(..2)
            .unwrap_or_default()
            .iter()
            .copied()
            .max()
            .unwrap_or(0);
        assert!(bass_max < 128, "同幅 sub-bass 段应显著低于中频段:{env:?}");
        assert_eq!(
            env.get(2..).unwrap_or_default().iter().max().copied(),
            Some(255),
            "中频段应是全曲最响:{env:?}"
        );
        Ok(())
    }

    /// K 计权的高频搁架:同幅度 3kHz(presence)段应高于 500Hz(中频)段
    /// (搁架 +4dB,归一后中频段被压到 255 以下)。
    #[test]
    fn treble_reads_louder_than_midrange() -> color_eyre::Result<()> {
        let mid = tone(500.0, 0.5, 8_000, 16_000);
        let treble = tone(3_000.0, 0.5, 8_000, 16_000);
        let env = envelope_from_samples(
            mid.into_iter().chain(treble),
            ch(1)?,
            rate(8_000)?,
            &params(pts(4)?),
        )
        .ok_or_else(|| color_eyre::eyre::eyre!("非空输入必须产出包络"))?;
        let mid_max = env
            .get(..2)
            .unwrap_or_default()
            .iter()
            .copied()
            .max()
            .unwrap_or(0);
        assert!(
            mid_max <= 230,
            "高频搁架应让同幅中频段低于 presence 段:{env:?}"
        );
        assert_eq!(
            env.get(2..).unwrap_or_default().iter().max().copied(),
            Some(255),
            "presence 段应是全曲最响:{env:?}"
        );
        Ok(())
    }

    /// 系数推导对齐权威:48kHz 下现场推导的两级滤波系数必须与 ITU-R BS.1770 规范
    /// 正文给出的 48kHz 系数表逐位一致(规范只给这一个采样率的表,其余靠推导)。
    #[test]
    fn kweighting_matches_bs1770_table_at_48khz() {
        let kw = super::KWeighting::new(48_000.0, &shelf_params(), &highpass_params());
        let close = |a: f64, b: f64| (a - b).abs() < 1e-10;
        assert!(close(kw.shelf.b0, 1.535_124_859_586_97), "shelf b0");
        assert!(close(kw.shelf.b1, -2.691_696_189_406_38), "shelf b1");
        assert!(close(kw.shelf.b2, 1.198_392_810_852_85), "shelf b2");
        assert!(close(kw.shelf.a1, -1.690_659_293_182_41), "shelf a1");
        assert!(close(kw.shelf.a2, 0.732_480_774_215_85), "shelf a2");
        assert!(close(kw.highpass.b0, 1.0), "highpass b0");
        assert!(close(kw.highpass.b1, -2.0), "highpass b1");
        assert!(close(kw.highpass.b2, 1.0), "highpass b2");
        assert!(close(kw.highpass.a1, -1.990_047_454_833_98), "highpass a1");
        assert!(close(kw.highpass.a2, 0.990_072_250_366_21), "highpass a2");
    }

    /// 整曲解码入口:真实 WAV(500Hz 方波、幅度线性渐强)解出定长 200 点 / 当前版本 /
    /// 非降形状且末点饱和——覆盖 open → decode → 滤波 → 分块 → 量化的完整链路。
    /// (方波用纯整数合成,规避 f32 → i16 的 `as` 转换;渐强不能用裸 ramp,直流会被
    /// 高通滤掉。)
    #[test]
    fn envelope_from_file_decodes_real_wav() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("crescendo.wav");
        // 2 秒 8kHz:每 8 帧翻转极性(500Hz 方波),幅度 = 帧号/8 → 0..2000。
        let samples = (0..16_000i32)
            .map(|i| {
                let amp = i / 8;
                let sign = if (i / 8) % 2 == 0 { 1 } else { -1 };
                i16::try_from(amp * sign)
            })
            .collect::<Result<Vec<i16>, _>>()?;
        mineral_test::write_wav(
            &path, &samples, /*channels*/ 1, /*sample_rate*/ 8_000,
        )?;

        let env = crate::envelope::envelope_from_file(&path, &params(pts(200)?))?;
        assert_eq!(env.points.len(), 200);
        assert_eq!(env.version, crate::envelope::ENVELOPE_VERSION);
        assert!(
            env.points.windows(2).all(|w| w.first() <= w.last()),
            "渐强 WAV 的包络必须非降"
        );
        assert_eq!(env.points.last().copied(), Some(255));
        Ok(())
    }

    /// 立体声先并帧再统计:左右反相恰好抵消 → 全 0。
    /// (逐样本直接统计 |s| 会错误地得到全 255,此用例专门锁死并帧次序。)
    #[test]
    fn stereo_averages_frames_before_stats() -> color_eyre::Result<()> {
        let samples = (0..8192usize).map(|i| if i % 2 == 0 { 1.0f32 } else { -1.0f32 });
        let env = envelope_from_samples(samples, ch(2)?, rate(8_000)?, &params(pts(4)?))
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
            let env = envelope_from_samples(
                samples.into_iter(),
                NonZeroU16::MIN,
                NonZeroU32::MIN.saturating_add(7_999),
                &params(point_count),
            )
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
            let env = envelope_from_samples(
                samples.into_iter(),
                NonZeroU16::MIN,
                NonZeroU32::MIN.saturating_add(7_999),
                &params(point_count),
            )
            .ok_or_else(|| TestCaseError::fail("非空输入必须产出包络"))?;
            prop_assert_eq!(env.iter().max().copied(), Some(255));
        }

        /// 整体缩放不变:全部样本同乘 0.5(二进制浮点下精确)形状不变——线性滤波
        /// 与均方都与缩放交换,包络表达相对起伏。
        #[test]
        fn envelope_is_scale_invariant(
            samples in prop::collection::vec(-1.0f32..1.0, 1..8192),
            point_count in (1usize..=64).prop_filter_map("非零点数", NonZeroUsize::new),
        ) {
            let original = envelope_from_samples(
                samples.iter().copied(),
                NonZeroU16::MIN,
                NonZeroU32::MIN.saturating_add(7_999),
                &params(point_count),
            );
            let halved = envelope_from_samples(
                samples.iter().map(|s| s * 0.5),
                NonZeroU16::MIN,
                NonZeroU32::MIN.saturating_add(7_999),
                &params(point_count),
            );
            prop_assert_eq!(original, halved);
        }
    }
}
