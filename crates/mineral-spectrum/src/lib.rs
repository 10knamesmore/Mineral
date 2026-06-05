//! 实时 PCM → 频谱条计算。caller 喂 mono f32 样本 + 期望条数,本 crate 出对应根数的高度
//! (0..=[`RES`])。FFT 大小 / 窗函数 / 桶映射 / dB 标定经 [`SpectrumParams`] 注入
//! (生产值来自用户配置 `tui.spectrum` 段),UI 端按 area.width 决定要几根条,
//! 本 crate 在 (sr, bar_count) 缓存桶映射,只在变化时重算。

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};

/// 单根条的最大高度(1/8 字符 × 8)。UI 渲染用同一个数。
/// 渲染结构契约,不进配置面。
pub const RES: u16 = 64;

/// FFT 窗大小的下限(样本)。再小桶映射在低频区退化到无意义。
const MIN_FFT_SIZE: usize = 64;

/// DSP 计算参数(来自用户配置 `tui.spectrum` 段;本 crate 保持叶子,不依赖配置 crate)。
///
/// **外键提醒**(参数间/跨段耦合,改值时同步):
/// - `fft_size` ↔ 音频侧 PCM tap 容量(配置 `audio.tap_capacity`):tap 须 ≥ 2 × `fft_size`,
///   否则 UI 卡一帧就环溢出丢样本,窗内出现时间不连续拼接 → 频谱毛刺。
/// - `fft_size` ↔ UI 每帧拉取量:消费端按 [`SpectrumComputer::window_size`] 取,自动跟随。
/// - `db_floor` < `db_ceil` 必须成立(违反时 [`SpectrumComputer`] 内部按最小间距 1dB 自救)。
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct SpectrumParams {
    /// FFT 窗大小(样本)。4096 在 48kHz 下 ≈ 85ms:低频 bin 间距 ~12Hz,细节足;
    /// 代价是瞬态被窗摊开、起播首窗延迟 ≈ 窗长。**建议 2 的幂**(否则 FFT 退化到
    /// 大常数路径);下限 [`MIN_FFT_SIZE`],构造时自动钳。
    fft_size: usize,

    /// 频率轴下界(Hz)。低于此频率的 bin 不参与桶映射。
    f_min: f32,

    /// 频率轴上界(Hz)。超过 SR/2(奈奎斯特)时取 SR/2。
    f_max: f32,

    /// 频率轴对数化程度,0.0(纯线性)..=1.0(纯对数,每 octave 等宽)。略小于 1
    /// 让低频每根条占更宽的频带、尽量各占一个 FFT bin,收掉低频「宽平顶」。
    log_axis_blend: f32,

    /// dB 标定下界:低于此映射到条高 0。与 `db_ceil` 共同决定显示动态范围,
    /// 抬高 = 砍掉安静细节、整体变矮;降低 = 噪声底也可见。
    db_floor: f32,

    /// dB 标定上界:高于此 clamp 到满高。必须 > `db_floor`。
    db_ceil: f32,

    /// 频带取值时峰值(max)的占比,0.0..=1.0,其余给均值:
    /// `(1 - peak_mix)·均值 + peak_mix·峰值`。0 = 纯均值(抹平)、1 = 纯峰值(最躁)。
    peak_mix: f32,
}

/// 频谱计算器:维持一个滑动窗口的 PCM 环形缓冲,UI 一调 [`Self::compute`] 就出最新一窗的条高。
///
/// 不持有 ringbuf consumer,push / compute 完全解耦,方便测试。
///
/// 大缓冲放堆上(boxed slice):struct 本体只占指针,move / clone 不搬数据。
pub struct SpectrumComputer {
    /// DSP 参数(构造时注入,生命周期内不变)。
    params: SpectrumParams,

    /// FFT 窗大小(= `params.fft_size` 钳到下限后的值,热路径免重复钳)。
    size: usize,

    /// realfft 计划。Arc 是因为 RealToComplex 是 trait object,SpectrumComputer 自身需要 Send/Sync。
    fft: Arc<dyn RealToComplex<f32>>,

    /// PCM 环形缓冲(长 `size`)。`write_idx` 处是下一个写入位置,也是当前窗的「最旧样本」位置。
    in_buf: Box<[f32]>,

    /// 下一个写入位置(0..size)。
    write_idx: usize,

    /// 累计写入样本数,达到 `size` 就算"窗满",可以 compute。
    filled: usize,

    /// FFT 输入 scratch(已加 Hann 窗,长 `size`)。每次 compute 复用,避免 alloc。
    fft_in: Box<[f32]>,

    /// FFT 输出复数 bin 缓冲(长 `size/2 + 1`)。
    fft_out: Box<[Complex32]>,

    /// 预算 Hann 窗(长 `size`)。
    hann: Box<[f32]>,

    /// 第 i 根条对应的 fft_out 区间 `[start, end)`。`(sr, bar_count)` 变化时重算。
    bar_bins: Vec<(usize, usize)>,

    /// 上次算 `bar_bins` 时用的 SR。0 = 还没算过。
    cached_sr: u32,

    /// 上次算 `bar_bins` 时用的 bar_count。0 = 还没算过。
    cached_bars: usize,
}

impl SpectrumComputer {
    /// 构造空计算器。FFT plan + Hann 窗在这里按 `params.fft_size` 一次性 cache。
    ///
    /// # Params:
    ///   - `params`: DSP 参数(配置 `tui.spectrum` 段派生;`fft_size` 钳到下限)
    pub fn new(params: SpectrumParams) -> Self {
        let size = (*params.fft_size()).max(MIN_FFT_SIZE);
        let bins = size / 2 + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(size);
        let mut hann = vec![0.0_f32; size].into_boxed_slice();
        for (i, slot) in hann.iter_mut().enumerate() {
            *slot = hann_at(i, size);
        }
        Self {
            params,
            size,
            fft,
            in_buf: vec![0.0_f32; size].into_boxed_slice(),
            write_idx: 0,
            filled: 0,
            fft_in: vec![0.0_f32; size].into_boxed_slice(),
            fft_out: vec![Complex32::new(0.0, 0.0); bins].into_boxed_slice(),
            hann,
            bar_bins: Vec::new(),
            cached_sr: 0,
            cached_bars: 0,
        }
    }

    /// 当前 FFT 窗大小(样本)。消费端据此决定每帧 PCM 拉取量(≥ 每帧新增样本数即可,
    /// 取窗大小则一帧能补满整窗,卡顿后追上最快)。
    pub fn window_size(&self) -> usize {
        self.size
    }

    /// 追加一批样本到环形缓冲。超过窗大小时自然覆盖最旧样本(环形语义)。
    pub fn push(&mut self, samples: &[f32]) {
        for s in samples {
            if let Some(slot) = self.in_buf.get_mut(self.write_idx) {
                *slot = *s;
            }
            self.write_idx = (self.write_idx + 1) % self.size;
            if self.filled < self.size {
                self.filled += 1;
            }
        }
    }

    /// 算一窗。
    ///
    /// # Params:
    ///   - `sample_rate`: 当前 PCM 来源的采样率(Hz)。变化时重算桶映射。
    ///   - `bar_count`: 期望的条数。变化时重算桶映射(对数等分到新条数)。
    ///
    /// # Return:
    ///   - `Some(Vec<u16>)`:窗满,出 `bar_count` 根条的高度。
    ///   - `None`:窗内样本数不足(刚开始播 / 切歌瞬间)或 `bar_count == 0`。
    pub fn compute(&mut self, sample_rate: u32, bar_count: usize) -> Option<Vec<u16>> {
        if self.filled < self.size || sample_rate == 0 || bar_count == 0 {
            return None;
        }
        if sample_rate != self.cached_sr || bar_count != self.cached_bars {
            self.bar_bins = compute_bar_bins(
                sample_rate,
                bar_count,
                self.size,
                &self.params,
                *self.params.log_axis_blend(),
            );
            self.cached_sr = sample_rate;
            self.cached_bars = bar_count;
        }

        // 环形缓冲的「最旧样本」就在 write_idx 位置,逻辑顺序是
        // [write_idx..size) 接 [0..write_idx)。Hann 加权一并写进 fft_in。
        for i in 0..self.size {
            let src_idx = (self.write_idx + i) % self.size;
            let src = self.in_buf.get(src_idx).copied().unwrap_or(0.0);
            let w = self.hann.get(i).copied().unwrap_or(0.0);
            if let Some(dst) = self.fft_in.get_mut(i) {
                *dst = src * w;
            }
        }

        // realfft process 出错就返 None(几乎不会发生,但避免 panic)。
        if self
            .fft
            .process(&mut self.fft_in, &mut self.fft_out)
            .is_err()
        {
            return None;
        }

        let mut bars = vec![0u16; bar_count];
        for (i, (lo, hi)) in self.bar_bins.iter().copied().enumerate() {
            let Some(bar) = bars.get_mut(i) else { continue };
            *bar = bin_band_to_height(&self.fft_out, lo, hi, self.size, &self.params);
        }
        Some(bars)
    }
}

/// 第 `i` 个 Hann 窗系数(窗长 `n`):`0.5 * (1 - cos(2π i / (n-1)))`。
#[allow(clippy::as_conversions)]
fn hann_at(i: usize, n: usize) -> f32 {
    let denom = (n.saturating_sub(1).max(1)) as f32;
    let x = std::f32::consts::PI * 2.0 * (i as f32) / denom;
    0.5 - 0.5 * x.cos()
}

/// 把 `[lo, hi)` 区间内的复数 bin 取 magnitude 的「均值 + 峰值」混合(占比 `peak_mix`),
/// 做 dB 标定后映射到 `0..=RES`。
///
/// 混合而非纯均值:峰值让频带里的尖刺「蹦」出来、动态更强;均值兜住整体能量,不至于像纯峰值那么躁。
#[allow(clippy::as_conversions)]
fn bin_band_to_height(
    bins: &[Complex32],
    lo: usize,
    hi: usize,
    fft_size: usize,
    params: &SpectrumParams,
) -> u16 {
    if hi <= lo {
        return 0;
    }
    let slice = bins.get(lo..hi).unwrap_or(&[]);
    if slice.is_empty() {
        return 0;
    }
    let (sum, peak) = slice.iter().fold((0.0_f32, 0.0_f32), |(s, p), c| {
        let mag = (c.re * c.re + c.im * c.im).sqrt();
        (s + mag, p.max(mag))
    });
    let peak_mix = params.peak_mix().clamp(0.0, 1.0);
    let mean = sum / (slice.len() as f32);
    let mixed = (1.0 - peak_mix) * mean + peak_mix * peak;
    // FFT 输出未归一化:除 fft_size/2 把 0..1 输入大致映射到 0..1 magnitude。
    let normalized = mixed / ((fft_size as f32) * 0.5);
    let db = 20.0 * (normalized + 1e-9).log10();
    // floor/ceil 写反时按最小间距 1dB 自救,避免除零 / 反向映射。
    let floor = *params.db_floor();
    let ceil = params.db_ceil().max(floor + 1.0);
    let clamped = db.clamp(floor, ceil);
    let frac = (clamped - floor) / (ceil - floor);
    let h = (frac * f32::from(RES)).round();
    if h <= 0.0 {
        0
    } else if h >= f32::from(RES) {
        RES
    } else {
        u16::try_from(h as i64).unwrap_or(0).min(RES)
    }
}

/// 在 `[f_min, min(f_max, sr/2)]` 上近对数等分 `bar_count` 个桶,把每个桶映射到 `[lo, hi)`
/// 的 bin index。频率轴的对数化程度由 `blend` 控制(线性↔对数插值)。
///
/// # Params:
///   - `sample_rate`: PCM 采样率(Hz)。
///   - `bar_count`: 期望条数。
///   - `fft_size`: FFT 窗大小(决定 bin 数与每 bin 频宽)。
///   - `params`: 频率轴界(`f_min` / `f_max`)。
///   - `blend`: 频率轴对数化程度,`1.0` 为纯对数,越小越偏线性。
///
/// # Return:
///   每根条对应的 `[lo, hi)` bin 区间。
#[allow(clippy::as_conversions)]
fn compute_bar_bins(
    sample_rate: u32,
    bar_count: usize,
    fft_size: usize,
    params: &SpectrumParams,
    blend: f32,
) -> Vec<(usize, usize)> {
    let nyquist = (sample_rate as f32) / 2.0;
    let f_hi = params.f_max().min(nyquist);
    let f_lo = params.f_min().min(f_hi);
    let log_lo = f_lo.max(1.0).ln();
    let log_hi = f_hi.max(f_lo + 1.0).ln();
    let bin_count = fft_size / 2 + 1;
    let hz_per_bin = (sample_rate as f32) / (fft_size as f32);

    // 归一化位置 t∈[0,1] → 频率:对数映射 exp(lerp(log_lo, log_hi)) 与线性映射按 blend 插值。
    let warp = |t: f32| {
        let log = (log_lo + (log_hi - log_lo) * t).exp();
        let lin = f_lo + (f_hi - f_lo) * t;
        (1.0 - blend) * lin + blend * log
    };

    let mut out = vec![(0usize, 0usize); bar_count];
    let bars_f = (bar_count as f32).max(1.0);
    for (i, slot) in out.iter_mut().enumerate() {
        let t0 = (i as f32) / bars_f;
        let t1 = ((i + 1) as f32) / bars_f;
        let f0 = warp(t0);
        let f1 = warp(t1);
        let b0 = (f0 / hz_per_bin).floor().max(0.0) as usize;
        let b1 = (f1 / hz_per_bin).ceil().max(0.0) as usize;
        let b0_clamped = b0.min(bin_count.saturating_sub(1));
        let b1_clamped = b1.clamp(b0_clamped + 1, bin_count);
        *slot = (b0_clamped, b1_clamped);
    }
    out
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use realfft::num_complex::Complex32;

    use super::{SpectrumComputer, SpectrumParams};

    /// 测试默认条数(跟原 `BARS` 常量一致,后续如果不再用 64 测试改这一处)。
    const TEST_BARS: usize = 64;

    /// 测试窗大小(与测试基线参数一致)。
    const FFT_SIZE: usize = 4096;

    /// 测试基线参数(任意合理值;生产默认的唯一真相源是 mineral-config 的 default.lua)。
    fn params() -> SpectrumParams {
        params_with_fft(FFT_SIZE)
    }

    /// 同 [`params`],但指定 `fft_size`。
    fn params_with_fft(fft_size: usize) -> SpectrumParams {
        SpectrumParams::builder()
            .fft_size(fft_size)
            .f_min(20.0)
            .f_max(20_000.0)
            .log_axis_blend(0.92)
            .db_floor(-65.0)
            .db_ceil(-6.0)
            .peak_mix(0.5)
            .build()
    }

    #[test]
    fn empty_compute_returns_none() -> color_eyre::Result<()> {
        let mut sc = SpectrumComputer::new(params());
        assert!(sc.compute(48_000, TEST_BARS).is_none());
        Ok(())
    }

    #[test]
    fn zero_signal_all_bars_zero() -> color_eyre::Result<()> {
        let mut sc = SpectrumComputer::new(params());
        sc.push(&vec![0.0_f32; FFT_SIZE]);
        let bars = sc
            .compute(48_000, TEST_BARS)
            .ok_or_else(|| eyre!("expected Some"))?;
        for (i, b) in bars.iter().enumerate() {
            assert_eq!(*b, 0, "bar {i} should be 0 for silence");
        }
        Ok(())
    }

    #[test]
    #[allow(clippy::as_conversions)]
    fn sine_peaks_around_target_bar() -> color_eyre::Result<()> {
        let sr: u32 = 48_000;
        let f: f32 = 1000.0;
        let mut samples = Vec::<f32>::with_capacity(FFT_SIZE);
        for i in 0..FFT_SIZE {
            let t = (i as f32) / (sr as f32);
            samples.push((2.0 * std::f32::consts::PI * f * t).sin());
        }
        let mut sc = SpectrumComputer::new(params());
        sc.push(&samples);
        let bars = sc
            .compute(sr, TEST_BARS)
            .ok_or_else(|| eyre!("expected Some"))?;
        let (peak_idx, &peak_val) = bars
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| **v)
            .ok_or_else(|| eyre!("empty bars"))?;
        // log_axis_blend=0.92(掺 8% 线性)把中频左移:1kHz 落 bar ~26(纯对数时 ~36,见
        // blend_shifts_* 锁方向)。范围按 blend 设计区间 [0.88,0.96] 的落点漂移取 22..=32。
        assert!(
            (22..=32).contains(&peak_idx),
            "1kHz peak expected mid-band (blend=0.92 ⇒ ~bar 26), got bar {peak_idx}"
        );
        assert!(peak_val > 0, "peak should be > 0");
        assert_eq!(bars.len(), TEST_BARS);
        Ok(())
    }

    /// `blend < 1` 应把低频铺开:1kHz 对应的 bin 在近对数轴上落到比纯对数更靠左的条。
    /// 锁住 `log_axis_blend` 的方向性,避免日后误调成 `> 1` 或把线性/对数插值写反。
    #[test]
    #[allow(clippy::as_conversions)]
    fn blend_shifts_mid_freq_left_vs_pure_log() -> color_eyre::Result<()> {
        let sr: u32 = 48_000;
        let bars: usize = 120;
        let params = params();
        let blended =
            super::compute_bar_bins(sr, bars, FFT_SIZE, &params, *params.log_axis_blend());
        let pure = super::compute_bar_bins(sr, bars, FFT_SIZE, &params, 1.0 /*blend*/);

        let hz_per_bin = (sr as f32) / (FFT_SIZE as f32);
        let target_bin = (1000.0 / hz_per_bin) as usize;
        // 第一根「上界 bin 越过 target」的条:bins 的 hi 随 index 单调不减,position 取最左。
        let first_reaching = |v: &[(usize, usize)]| {
            v.iter()
                .position(|(_, hi)| *hi > target_bin)
                .ok_or_else(|| eyre!("no bar reaches bin {target_bin}"))
        };
        let i_blend = first_reaching(&blended)?;
        let i_pure = first_reaching(&pure)?;
        assert!(
            i_blend < i_pure,
            "blend 应让 1kHz 落到更靠左的条:blend@{i_blend} 未小于 pure@{i_pure}"
        );
        Ok(())
    }

    /// 同样总能量,集中在一个 bin vs 均摊到整带:含峰值的混合统计应让「集中」的更高。
    /// 纯均值会让两者等高 —— 这条锁住 `bin_band_to_height` 用的是「均值 + 峰值」混合而非纯均值。
    #[test]
    fn mixed_stat_rewards_spiky_band_over_flat() -> color_eyre::Result<()> {
        let n: usize = 16;
        let mut spiky = vec![Complex32::new(0.0, 0.0); n];
        if let Some(c) = spiky.get_mut(0) {
            *c = Complex32::new(1024.0, 0.0);
        }
        let flat = vec![Complex32::new(64.0, 0.0); n];
        let params = params();
        let h_spiky = super::bin_band_to_height(&spiky, 0 /*lo*/, n, FFT_SIZE, &params);
        let h_flat = super::bin_band_to_height(&flat, 0 /*lo*/, n, FFT_SIZE, &params);
        assert!(
            h_spiky > h_flat,
            "混合统计应奖励集中能量:spiky {h_spiky} 未高于 flat {h_flat}"
        );
        Ok(())
    }

    /// `fft_size` 注入生效:非默认窗大小下 `window_size` 跟随、窗满判定按新值走。
    #[test]
    fn custom_fft_size_takes_effect() -> color_eyre::Result<()> {
        let mut sc = SpectrumComputer::new(params_with_fft(1024));
        assert_eq!(sc.window_size(), 1024);
        sc.push(&vec![0.1_f32; 1023]);
        assert!(sc.compute(48_000, TEST_BARS).is_none(), "差 1 样本不应出窗");
        sc.push(&[0.1_f32]);
        assert!(sc.compute(48_000, TEST_BARS).is_some(), "窗满应出值");
        Ok(())
    }

    /// 过小的 `fft_size` 被钳到下限,不 panic、能正常出窗。
    #[test]
    fn tiny_fft_size_is_clamped() -> color_eyre::Result<()> {
        let mut sc = SpectrumComputer::new(params_with_fft(1));
        assert_eq!(sc.window_size(), super::MIN_FFT_SIZE, "应钳到下限");
        sc.push(&vec![0.0_f32; super::MIN_FFT_SIZE]);
        assert!(sc.compute(48_000, TEST_BARS).is_some());
        Ok(())
    }
}
