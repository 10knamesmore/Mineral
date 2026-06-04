//! 封面色板:从一张封面聚出的若干重点色,沿一维位置轴(频谱里是频率)插值取色。
//!
//! 渲染层与取色 worker 共用的纯色彩基元——只持有 [`Rgb`] 三元组,**不依赖**
//! `palette` / `kmeans_colors`(Lab 聚类只发生在 worker 的 `extract_palette` 里)。
//! 故本模块与频谱状态机解耦,可独立单测。

use ratatui::style::Color;

use crate::render::color::lerp_byte;

/// 一个 sRGB 颜色(色板 swatch / 取色结果的结构化承载)。
///
/// 不用裸 `(u8,u8,u8)` 元组:命名字段让 swatch / 端点的语义在调用点自解释。
/// 不依赖 ratatui `Color`,保持本类型可被取色 worker 与渲染层共用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    /// 红分量。
    pub r: u8,

    /// 绿分量。
    pub g: u8,

    /// 蓝分量。
    pub b: u8,
}

impl Rgb {
    /// 构造一个 sRGB 颜色。
    ///
    /// # Params:
    ///   - `r` / `g` / `b`: 各分量(0..=255)
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// 转成 ratatui 渲染色。
    fn to_color(self) -> Color {
        Color::Rgb(self.r, self.g, self.b)
    }

    /// 在两个 swatch 之间按 `num/denom` 逐分量整数 lerp,返回渲染色。
    ///
    /// # Params:
    ///   - `to`: 终点色
    ///   - `num` / `denom`: lerp 比例(`num` 越界自动 clamp 到 `denom`)
    fn lerp_to(self, to: Self, num: u64, denom: u64) -> Color {
        Color::Rgb(
            lerp_byte(self.r, to.r, num, denom),
            lerp_byte(self.g, to.g, num, denom),
            lerp_byte(self.b, to.b, num, denom),
        )
    }
}

/// 一列频谱柱的底 / 顶端点色(垂直渐变的两端)。命名字段替代 `(Color, Color)` 元组。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnColors {
    /// 底端点(柱底、低频侧)。
    pub bottom: Color,

    /// 顶端点(柱顶,沿色带偏高频处采样)。
    pub top: Color,
}

/// 一张封面提取出的色板:按 Lab 明度升序排列的若干重点色。
///
/// 频谱用它沿频率轴铺色(低频取暗色、高频取亮色);[`Self::sample`] 把
/// `0..=1000` 的位置映射到这条色带上插值取色。
#[derive(Clone, Debug)]
pub struct CoverPalette {
    /// 重点色,Lab 明度升序;长度 = 实际聚出的色数(≤ k),恒非空(由 [`Self::new`] 保证)。
    swatches: Vec<Rgb>,
}

impl CoverPalette {
    /// 从明度升序的重点色构造。空 `swatches` 返回 `None`——无色的色板无意义,
    /// 调用方据此回退(频谱回 hue 漂移)。
    ///
    /// # Params:
    ///   - `swatches`: 重点色(应已按 Lab 明度升序)
    ///
    /// # Return:
    ///   非空时 `Some(CoverPalette)`,空时 `None`。
    pub fn new(swatches: Vec<Rgb>) -> Option<Self> {
        if swatches.is_empty() {
            None
        } else {
            Some(Self { swatches })
        }
    }

    /// 重点色切片(Lab 明度升序)。
    ///
    /// `sample` / `column_endpoints` 内部直接读字段,故非 test 构建暂无生产消费者;
    /// 由跨模块测试(`cover_colors` 断言取色结果)与未来"频谱外取色复用"读取。
    #[allow(dead_code)] // reason: 见上,getter 当前仅测试 / 未来消费者用
    pub fn swatches(&self) -> &[Rgb] {
        &self.swatches
    }

    /// 把位置 `pos_permille ∈ 0..=1000`(整数定点,越界自动 clamp)映射到色带上取色。
    ///
    /// `0` 命中首个(最暗)swatch、`1000` 命中末个(最亮)swatch,中间在相邻两个 swatch
    /// 之间逐分量整数 lerp。单 swatch 色板任意位置都返回那一个色。
    ///
    /// # Params:
    ///   - `pos_permille`: 色带位置千分比(`0..=1000`)
    ///
    /// # Return:
    ///   对应位置的 `Color::Rgb`。
    pub fn sample(&self, pos_permille: u32) -> Color {
        let pos = pos_permille.min(1000);
        // segments = swatch 数 - 1 = 色带被分成的区间数。
        let segments = u32::try_from(self.swatches.len())
            .unwrap_or(1)
            .saturating_sub(1);
        if segments == 0 {
            // 单 swatch:整条色带一个颜色。
            return match self.swatches.first() {
                Some(swatch) => swatch.to_color(),
                None => Color::Reset,
            };
        }
        // pos 落在第 idx 段 [idx/segments, (idx+1)/segments],末段封顶到 segments-1。
        let idx = (pos.saturating_mul(segments) / 1000).min(segments - 1);
        let seg_lo = idx.saturating_mul(1000) / segments;
        let seg_hi = (idx + 1).saturating_mul(1000) / segments;
        let num = u64::from(pos.saturating_sub(seg_lo));
        let denom = u64::from(seg_hi.saturating_sub(seg_lo).max(1));
        let lo_i = usize::try_from(idx).unwrap_or(0);
        match (self.swatches.get(lo_i), self.swatches.get(lo_i + 1)) {
            (Some(lo), Some(hi)) => lo.lerp_to(*hi, num, denom),
            // idx+1 恒 ≤ segments = len-1 < len,故不可达;留作类型穷尽。
            _ => Color::Reset,
        }
    }

    /// 算第 `col` 列(共 `bar_count` 列)的底/顶端点色,构成一块连续 2D 色场。
    ///
    /// 横轴 = 频率位置 `tx‰ = col * 1000 / (bar_count-1)`(最左低频 `0`、最右高频 `1000`)。
    /// 底端点 = `sample(tx)`;顶端点 = `sample(tx + vshift_permille)`——沿同一条色带略偏
    /// 高频处采样,制造纵向层次;封顶 `1000`(最高频列顶色 clamp 到末 swatch)。
    ///
    /// # Params:
    ///   - `col`: 列序号(从 0 起)
    ///   - `bar_count`: 总列数(`<= 1` 时 `tx` 取 0,避免除零)
    ///   - `vshift_permille`: 顶端点相对底端点沿色带的高频偏移(‰)
    ///
    /// # Return:
    ///   该列的底 / 顶端点色。
    pub fn column_endpoints(
        &self,
        col: usize,
        bar_count: usize,
        vshift_permille: u32,
    ) -> ColumnColors {
        let tx = column_permille(col, bar_count);
        ColumnColors {
            bottom: self.sample(tx),
            top: self.sample(tx.saturating_add(vshift_permille).min(1000)),
        }
    }
}

/// 把列序号映射到色带位置千分比:`col * 1000 / (bar_count-1)`,最左 `0`、最右 `1000`。
/// `bar_count <= 1` 时返回 `0`(单列退化,避免除零)。整数定点,无 `as`。
///
/// # Params:
///   - `col`: 列序号(从 0 起)
///   - `bar_count`: 总列数
///
/// # Return:
///   位置千分比 `0..=1000`。
pub(crate) fn column_permille(col: usize, bar_count: usize) -> u32 {
    if bar_count <= 1 {
        return 0;
    }
    let col = u32::try_from(col).unwrap_or(0);
    let denom = u32::try_from(bar_count - 1).unwrap_or(1).max(1);
    (col.saturating_mul(1000) / denom).min(1000)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::{CoverPalette, Rgb};

    /// 三色板:`sample(0)` 命中首 swatch、`sample(1000)` 命中末 swatch。
    #[test]
    fn sample_hits_endpoints() -> color_eyre::Result<()> {
        let pal = CoverPalette::new(vec![
            Rgb::new(10, 20, 30),
            Rgb::new(100, 110, 120),
            Rgb::new(200, 210, 220),
        ])
        .ok_or_else(|| color_eyre::eyre::eyre!("非空应构造成功"))?;
        assert_eq!(pal.sample(0), Color::Rgb(10, 20, 30));
        assert_eq!(pal.sample(1000), Color::Rgb(200, 210, 220));
        Ok(())
    }

    /// 单 swatch 色板:任意位置都返回那一个色,且恒为 `Rgb`。
    #[test]
    fn sample_single_swatch_constant() -> color_eyre::Result<()> {
        let pal = CoverPalette::new(vec![Rgb::new(42, 43, 44)])
            .ok_or_else(|| color_eyre::eyre::eyre!("非空应构造成功"))?;
        assert_eq!(pal.sample(0), Color::Rgb(42, 43, 44));
        assert_eq!(pal.sample(500), Color::Rgb(42, 43, 44));
        assert_eq!(pal.sample(1000), Color::Rgb(42, 43, 44));
        Ok(())
    }

    /// 空 `swatches` 构造返回 `None`。
    #[test]
    fn new_rejects_empty() {
        assert!(CoverPalette::new(Vec::new()).is_none());
    }

    /// `column_endpoints`:最左列底色 = `sample(0)`、最右列底色 = `sample(1000)`,
    /// 顶色沿色带偏高频(此处仍命中末 swatch)。
    #[test]
    fn column_endpoints_span_frequency_axis() -> color_eyre::Result<()> {
        let pal = CoverPalette::new(vec![
            Rgb::new(0, 0, 0),
            Rgb::new(128, 128, 128),
            Rgb::new(255, 255, 255),
        ])
        .ok_or_else(|| color_eyre::eyre::eyre!("非空应构造成功"))?;
        let first = pal.column_endpoints(/*col*/ 0, /*bar_count*/ 4, /*vshift*/ 200);
        let last = pal.column_endpoints(/*col*/ 3, /*bar_count*/ 4, /*vshift*/ 200);
        assert_eq!(first.bottom, pal.sample(0), "最左列底色应是色带起点");
        assert_eq!(last.bottom, pal.sample(1000), "最右列底色应是色带终点");
        assert_eq!(
            last.top,
            pal.sample(1000),
            "最右列顶色偏移后 clamp 到末 swatch"
        );
        Ok(())
    }

    /// `bar_count <= 1` 时不除零:单列底色取色带起点。
    #[test]
    fn column_endpoints_single_column_no_div_by_zero() -> color_eyre::Result<()> {
        let pal = CoverPalette::new(vec![Rgb::new(1, 2, 3), Rgb::new(4, 5, 6)])
            .ok_or_else(|| color_eyre::eyre::eyre!("非空应构造成功"))?;
        let only = pal.column_endpoints(/*col*/ 0, /*bar_count*/ 1, /*vshift*/ 200);
        assert_eq!(only.bottom, pal.sample(0));
        Ok(())
    }

    use proptest::collection::vec as pvec;
    use proptest::prelude::{any, proptest};

    proptest! {
        /// 任意非空色板 + 任意位置:`sample` 恒返回 `Color::Rgb`,不 panic、不掉 `Reset`。
        #[test]
        fn sample_always_rgb(
            comps in pvec((any::<u8>(), any::<u8>(), any::<u8>()), 1..=6),
            pos in any::<u32>(),
        ) {
            let swatches = comps.into_iter().map(|(r, g, b)| Rgb::new(r, g, b)).collect::<Vec<Rgb>>();
            if let Some(pal) = CoverPalette::new(swatches) {
                proptest::prop_assert!(matches!(pal.sample(pos), Color::Rgb(..)));
            }
        }
    }
}
