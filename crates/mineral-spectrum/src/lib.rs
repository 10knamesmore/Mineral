//! 实时 PCM → 频谱条计算。caller 喂 mono f32 样本 + 期望条数,本 crate 出对应根数的高度
//! (0..=[`RES`])。FFT 大小 / 窗函数 / 桶映射 / dB 标定全封装在内,UI 端按 area.width
//! 决定要几根条,本 crate 在 (sr, bar_count) 缓存桶映射,只在变化时重算。

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};

/// 单根条的最大高度(1/8 字符 × 8)。UI 渲染用同一个数。
pub const RES: u16 = 64;

/// FFT 窗大小(样本)。2048 在 48kHz 下 ≈ 43ms,UI 33ms 一帧总有新数据。
const FFT_SIZE: usize = 2048;

/// FFT 输出复数 bin 数(real-input FFT 只产正频)。
const FFT_BINS: usize = FFT_SIZE / 2 + 1;

/// 频率轴下界(Hz)。低于此频率的 bin 不参与桶映射。
const F_MIN: f32 = 20.0;

/// 频率轴上界(Hz)。SR/2 不足时取 SR/2。
const F_MAX: f32 = 20_000.0;

/// dB 标定下界。低于此值映射到 0。
const DB_FLOOR: f32 = -60.0;

/// dB 标定上界。高于此值 clamp 到顶。
const DB_CEIL: f32 = 0.0;

/// 频谱计算器:维持一个滑动窗口的 PCM 环形缓冲,UI 一调 [`Self::compute`] 就出最新一窗的条高。
///
/// 不持有 ringbuf consumer,push / compute 完全解耦,方便测试。
///
/// 大数组放进 `Box`:struct 本体只占指针,move / clone 不搬 16+KB 数据。
pub struct SpectrumComputer {
    fft: Arc<dyn RealToComplex<f32>>,

    /// PCM 环形缓冲。`write_idx` 处是下一个写入位置,也是当前窗的「最旧样本」位置。
    in_buf: Box<[f32; FFT_SIZE]>,

    /// 下一个写入位置(0..FFT_SIZE)。
    write_idx: usize,

    /// 累计写入样本数,达到 FFT_SIZE 就算"窗满",可以 compute。
    filled: usize,

    /// FFT 输入 scratch(已加 Hann 窗)。每次 compute 复用,避免 alloc。
    fft_in: Box<[f32; FFT_SIZE]>,

    /// FFT 输出复数 bin 缓冲。
    fft_out: Box<[Complex32; FFT_BINS]>,

    /// 预算 Hann 窗。
    hann: Box<[f32; FFT_SIZE]>,

    /// 第 i 根条对应的 fft_out 区间 `[start, end)`。`(sr, bar_count)` 变化时重算。
    bar_bins: Vec<(usize, usize)>,

    /// 上次算 `bar_bins` 时用的 SR。0 = 还没算过。
    cached_sr: u32,

    /// 上次算 `bar_bins` 时用的 bar_count。0 = 还没算过。
    cached_bars: usize,
}

impl SpectrumComputer {
    /// 构造空计算器。FFT plan + Hann 窗在这里一次性 cache。
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let mut hann = Box::new([0.0_f32; FFT_SIZE]);
        for (i, slot) in hann.iter_mut().enumerate() {
            *slot = hann_at(i);
        }
        Self {
            fft,
            in_buf: Box::new([0.0_f32; FFT_SIZE]),
            write_idx: 0,
            filled: 0,
            fft_in: Box::new([0.0_f32; FFT_SIZE]),
            fft_out: Box::new([Complex32::new(0.0, 0.0); FFT_BINS]),
            hann,
            bar_bins: Vec::new(),
            cached_sr: 0,
            cached_bars: 0,
        }
    }

    /// 追加一批样本到环形缓冲。超过窗大小时自然覆盖最旧样本(环形语义)。
    pub fn push(&mut self, samples: &[f32]) {
        for s in samples {
            if let Some(slot) = self.in_buf.get_mut(self.write_idx) {
                *slot = *s;
            }
            self.write_idx = (self.write_idx + 1) % FFT_SIZE;
            if self.filled < FFT_SIZE {
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
    ///   - `None`:窗内样本数 < FFT_SIZE(刚开始播 / 切歌瞬间)或 `bar_count == 0`。
    pub fn compute(&mut self, sample_rate: u32, bar_count: usize) -> Option<Vec<u16>> {
        if self.filled < FFT_SIZE || sample_rate == 0 || bar_count == 0 {
            return None;
        }
        if sample_rate != self.cached_sr || bar_count != self.cached_bars {
            self.bar_bins = compute_bar_bins(sample_rate, bar_count);
            self.cached_sr = sample_rate;
            self.cached_bars = bar_count;
        }

        // 环形缓冲的「最旧样本」就在 write_idx 位置,逻辑顺序是
        // [write_idx..FFT_SIZE) 接 [0..write_idx)。Hann 加权一并写进 fft_in。
        for i in 0..FFT_SIZE {
            let src_idx = (self.write_idx + i) % FFT_SIZE;
            let src = self.in_buf.get(src_idx).copied().unwrap_or(0.0);
            let w = self.hann.get(i).copied().unwrap_or(0.0);
            if let Some(dst) = self.fft_in.get_mut(i) {
                *dst = src * w;
            }
        }

        // realfft process 出错就返 None(几乎不会发生,但避免 panic)。
        if self
            .fft
            .process(self.fft_in.as_mut_slice(), self.fft_out.as_mut_slice())
            .is_err()
        {
            return None;
        }

        let mut bars = vec![0u16; bar_count];
        for (i, (lo, hi)) in self.bar_bins.iter().copied().enumerate() {
            let Some(bar) = bars.get_mut(i) else { continue };
            *bar = bin_band_to_height(self.fft_out.as_slice(), lo, hi);
        }
        Some(bars)
    }
}

impl Default for SpectrumComputer {
    fn default() -> Self {
        Self::new()
    }
}

/// 第 `i` 个 Hann 窗系数:`0.5 * (1 - cos(2π i / (N-1)))`。
#[allow(clippy::as_conversions)]
fn hann_at(i: usize) -> f32 {
    let n = (FFT_SIZE - 1) as f32;
    let x = std::f32::consts::PI * 2.0 * (i as f32) / n;
    0.5 - 0.5 * x.cos()
}

/// 把 `[lo, hi)` 区间内的复数 bin 算 magnitude 均值,做 dB 标定后映射到 `0..=RES`。
#[allow(clippy::as_conversions)]
fn bin_band_to_height(bins: &[Complex32], lo: usize, hi: usize) -> u16 {
    if hi <= lo {
        return 0;
    }
    let slice = bins.get(lo..hi).unwrap_or(&[]);
    if slice.is_empty() {
        return 0;
    }
    let sum: f32 = slice
        .iter()
        .map(|c| (c.re * c.re + c.im * c.im).sqrt())
        .sum();
    let mean = sum / (slice.len() as f32);
    // FFT 输出未归一化:除 FFT_SIZE/2 把 0..1 输入大致映射到 0..1 magnitude。
    let normalized = mean / ((FFT_SIZE as f32) * 0.5);
    let db = 20.0 * (normalized + 1e-9).log10();
    let clamped = db.clamp(DB_FLOOR, DB_CEIL);
    let frac = (clamped - DB_FLOOR) / (DB_CEIL - DB_FLOOR);
    let h = (frac * f32::from(RES)).round();
    if h <= 0.0 {
        0
    } else if h >= f32::from(RES) {
        RES
    } else {
        u16::try_from(h as i64).unwrap_or(0).min(RES)
    }
}

/// 在 `[F_MIN, min(F_MAX, sr/2)]` 上对数等分 `bar_count` 个桶,把每个桶映射到 `[lo, hi)` 的 bin index。
#[allow(clippy::as_conversions)]
fn compute_bar_bins(sample_rate: u32, bar_count: usize) -> Vec<(usize, usize)> {
    let nyquist = (sample_rate as f32) / 2.0;
    let f_hi = F_MAX.min(nyquist);
    let f_lo = F_MIN.min(f_hi);
    let log_lo = f_lo.max(1.0).ln();
    let log_hi = f_hi.max(f_lo + 1.0).ln();
    let bin_count = FFT_SIZE / 2 + 1;
    let hz_per_bin = (sample_rate as f32) / (FFT_SIZE as f32);

    let mut out = vec![(0usize, 0usize); bar_count];
    let bars_f = (bar_count as f32).max(1.0);
    for (i, slot) in out.iter_mut().enumerate() {
        let t0 = (i as f32) / bars_f;
        let t1 = ((i + 1) as f32) / bars_f;
        let f0 = (log_lo + (log_hi - log_lo) * t0).exp();
        let f1 = (log_lo + (log_hi - log_lo) * t1).exp();
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

    use super::{FFT_SIZE, SpectrumComputer};

    /// 测试默认条数(跟原 `BARS` 常量一致,后续如果不再用 64 测试改这一处)。
    const TEST_BARS: usize = 64;

    #[test]
    fn empty_compute_returns_none() -> color_eyre::Result<()> {
        let mut sc = SpectrumComputer::new();
        assert!(sc.compute(48_000, TEST_BARS).is_none());
        Ok(())
    }

    #[test]
    fn zero_signal_all_bars_zero() -> color_eyre::Result<()> {
        let mut sc = SpectrumComputer::new();
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
        let mut sc = SpectrumComputer::new();
        sc.push(&samples);
        let bars = sc
            .compute(sr, TEST_BARS)
            .ok_or_else(|| eyre!("expected Some"))?;
        let (peak_idx, &peak_val) = bars
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| **v)
            .ok_or_else(|| eyre!("empty bars"))?;
        assert!(
            (28..=44).contains(&peak_idx),
            "1kHz peak expected mid-band, got bar {peak_idx}"
        );
        assert!(peak_val > 0, "peak should be > 0");
        assert_eq!(bars.len(), TEST_BARS);
        Ok(())
    }
}
