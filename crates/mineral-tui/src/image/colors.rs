//! 封面取色:把一张已解码的封面图聚成若干重点色,沿 Lab 明度升序排成 [`CoverPalette`]。
//!
//! 跑在 cover fetcher worker 的 `spawn_blocking` 里(CPU 密集,与解码同处),色带随图回传,
//! **不在渲染线程现算**。取色是封面的附属信息,尽力而为:任一步失败返回 `None`,
//! 频谱回退 hue 漂移,封面图本身照常显示。

use image::DynamicImage;
use kmeans_colors::{Sort, get_kmeans};
use palette::cast::from_component_slice;
use palette::convert::FromColorUnclamped;
use palette::{FromColor, IntoColor, Lab, Lch, Srgb};

use crate::render::accent::AccentPair;
use crate::render::palette::{CoverPalette, Rgb};

use super::resize::thumbnail;

/// 从一张封面图提取频谱色板(Lab 明度升序的重点色)。
///
/// 流程:缩到 `kmeans.sample_dim` 采样图(取色不需要全分辨率;box filter 确定性 +
/// 固定 seed,取色仍确定,色板数值与全分辨率聚类略有差异、频谱配色视觉无感)→ 像素整块转 Lab →
/// 丢近黑/近白/近灰(有效像素过少则回退不过滤)→ k-means 聚类(固定 seed,确定性)
/// → 按 Lab 明度升序转回 sRGB。
///
/// # Params:
///   - `img`: 已解码的完整封面图
///   - `k`: k-means 取色旋钮(配置 `tui.cover.kmeans` 段)
///
/// # Return:
///   `Some(CoverPalette)`;尺寸为 0 / 有效像素为 0 / 聚类无结果时 `None`。
pub fn extract_palette(
    img: &DynamicImage,
    k: &mineral_config::KmeansConfig,
) -> Option<CoverPalette> {
    // 大图先降采样:聚类只看颜色分布,sample_dim² 样本足够;box filter 极快且确定。
    let dim = (*k.sample_dim()).max(1);
    let rgb = if img.width() > dim || img.height() > dim {
        thumbnail(img, dim, dim).into_rgb8()
    } else {
        img.to_rgb8()
    };
    if rgb.width() == 0 || rgb.height() == 0 {
        return None;
    }
    // 整块 RGB 字节零拷贝重解释成 `Srgb<u8>`,逐个转 Lab 感知色空间。
    let all_lab: Vec<Lab> = from_component_slice::<Srgb<u8>>(rgb.as_raw().as_slice())
        .iter()
        .map(|px| px.into_format::<f32>().into_color())
        .collect();
    if all_lab.is_empty() {
        return None;
    }

    // 丢近黑 / 近白 / 近灰,避免背景色霸占色板。
    let filtered: Vec<Lab> = all_lab
        .iter()
        .copied()
        .filter(|lab| is_vivid(lab, k))
        .collect();
    // 有效像素太少(黑白 / 低饱和封面)则不过滤,用全部像素兜底。
    let valid_pct = filtered.len().saturating_mul(100) / all_lab.len();
    let samples: &[Lab] = if valid_pct < *k.min_valid_pixels_pct() {
        &all_lab
    } else {
        &filtered
    };
    if samples.is_empty() {
        return None;
    }

    let result = get_kmeans(
        *k.swatches(),
        *k.max_iter(),
        *k.converge(),
        false, /*verbose*/
        samples,
        *k.seed(),
    );
    // `sort_indexed_colors` 内置丢弃空簇 + 按 L 暗→亮排序,正好是"低频暗→高频亮"。
    let sorted = Lab::sort_indexed_colors(&result.centroids, &result.indices);
    let swatches: Vec<Rgb> = sorted.iter().map(|cd| lab_to_rgb(cd.centroid)).collect();
    CoverPalette::new(swatches)
}

/// accent 派生色的明度 clamp 下限(Lab L):再低在深色背景上晦暗难辨。
const ACCENT_L_MIN: f32 = 55.0;

/// accent 派生色的明度 clamp 上限(Lab L):再高泛白失彩,强调感消失。
const ACCENT_L_MAX: f32 = 78.0;

/// accent 派生色的彩度下限:近灰封面(黑白 / 低饱和)派生出的强调色提到此彩度,
/// 保留一丝可辨的色相倾向而不至于像禁用态的灰。
const ACCENT_CHROMA_MIN: f32 = 16.0;

/// 主 / 副强调色的最小色相距离(度):色板里拉不开就由主色旋转派生副色。
const HUE_SEPARATION_DEG: f32 = 50.0;

/// 从封面色板派生一对强调色(主 = 最鲜艳簇,副 = 与主色相拉开的次鲜艳簇)。
///
/// 两色都经可读性整形:Lab 明度 clamp 进 [`ACCENT_L_MIN`]..=[`ACCENT_L_MAX`]、
/// 彩度保底 [`ACCENT_CHROMA_MIN`]、出 sRGB 色域则保明度/色相收彩度。色板内
/// 无色相距离 ≥ [`HUE_SEPARATION_DEG`] 的候选(单色 / 同色系封面)时,副色由
/// 主色旋转 [`HUE_SEPARATION_DEG`] 派生,保持同源和谐。纯函数、确定性,
/// 每次封面切换在 app 层调用一次(6 色以内,开销可忽略)。
///
/// # Params:
///   - `palette`: 封面色板(恒非空)
///
/// # Return:
///   派生的强调色对。
pub fn derive_accents(palette: &CoverPalette) -> AccentPair {
    // 色板恒非空(CoverPalette::new 保证),fallback 仅兜类型穷尽:中性紫灰。
    let fallback = Lch::new(65.0, 20.0, 300.0);
    let lchs: Vec<Lch> = palette.swatches().iter().map(|c| lch_of(*c)).collect();
    let seed = lchs
        .iter()
        .copied()
        .reduce(|a, b| if b.chroma > a.chroma { b } else { a })
        .unwrap_or(fallback);
    let second = lchs
        .iter()
        .copied()
        .filter(|c| hue_distance(*c, seed) >= HUE_SEPARATION_DEG)
        .reduce(|a, b| if b.chroma > a.chroma { b } else { a })
        .unwrap_or_else(|| {
            Lch::new(
                seed.l,
                seed.chroma,
                seed.hue.into_positive_degrees() + HUE_SEPARATION_DEG,
            )
        });
    AccentPair {
        accent: readable_rgb(seed),
        accent_2: readable_rgb(second),
    }
}

/// 把一个 Lch 候选整形成可读的强调色:明度 clamp + 彩度保底 + 色域内收。
fn readable_rgb(lch: Lch) -> Rgb {
    rgb_in_gamut(
        lch.l.clamp(ACCENT_L_MIN, ACCENT_L_MAX),
        lch.chroma.max(ACCENT_CHROMA_MIN),
        lch.hue.into_positive_degrees(),
    )
}

/// 保明度 / 色相、逐步收彩度直到落进 sRGB 色域(经典 gamut mapping)。
/// 彩度收到 0 即灰轴,恒在域内,循环必然终止。
///
/// 探测必须走 `from_color_unclamped`:`FromColor` 会把出域分量静默 clamp 进
/// [0, 1],探测永远"在域内",实际输出却被 clamp 改掉明度 / 色相(绿封面的
/// 副色曾因此亮出区间)。
fn rgb_in_gamut(l: f32, chroma: f32, hue_deg: f32) -> Rgb {
    let mut c = chroma;
    let mut candidate = Srgb::<f32>::from_color_unclamped(Lch::new(l, c, hue_deg));
    while !in_srgb_bounds(candidate) && c > 0.5 {
        c *= 0.9;
        candidate = Srgb::<f32>::from_color_unclamped(Lch::new(l, c, hue_deg));
    }
    // 极端退出(c 见底仍出域,理论不可达)与浮点毛刺由 clamped 转换兜底。
    let srgb: Srgb<u8> = Srgb::from_color(Lch::new(l, c, hue_deg)).into_format();
    Rgb::new(srgb.red, srgb.green, srgb.blue)
}

/// 三通道都在 [0, 1] 内(未越出 sRGB 色域)。
fn in_srgb_bounds(c: Srgb<f32>) -> bool {
    let ok = |v: f32| (0.0..=1.0).contains(&v);
    ok(c.red) && ok(c.green) && ok(c.blue)
}

/// swatch → Lch(明度 / 彩度 / 色相,选色的工作色空间)。
fn lch_of(c: Rgb) -> Lch {
    Lch::from_color(lab_of(c))
}

/// swatch → Lab(感知均匀插值的工作色空间)。
fn lab_of(c: Rgb) -> Lab {
    Srgb::new(c.r, c.g, c.b).into_format::<f32>().into_color()
}

/// 两个 sRGB 色在 **Lab 空间**按千分比插值:感知均匀,互补色中途不塌成灰
/// (RGB 直插会——两端彩度都高、中点却近灰轴)。氛围渐变的锚点色过渡用它。
///
/// # Params:
///   - `a` / `b`: 两端颜色
///   - `permille`: 插值位置千分比 `0..=1000`(越界 clamp;`0` = `a`、`1000` = `b`)
///
/// # Return:
///   插值后的 sRGB 颜色。
pub(crate) fn lerp_lab(a: Rgb, b: Rgb, permille: u16) -> Rgb {
    if permille == 0 {
        return a;
    }
    if permille >= 1000 {
        return b;
    }
    let t = f32::from(permille) / 1000.0;
    let (la, lb) = (lab_of(a), lab_of(b));
    let mixed = Lab::new(
        la.l + (lb.l - la.l) * t,
        la.a + (lb.a - la.a) * t,
        la.b + (lb.b - la.b) * t,
    );
    lab_to_rgb(mixed)
}

/// 两个 Lch 的环形色相距离(度,`0..=180`)。
fn hue_distance(a: Lch, b: Lch) -> f32 {
    let d = (a.hue.into_positive_degrees() - b.hue.into_positive_degrees()).abs() % 360.0;
    d.min(360.0 - d)
}

/// 像素是否"有色":明度在 `l_min..=l_max` 且彩度 ≥ `chroma_min`(配置 kmeans 段)。
///
/// # Params:
///   - `lab`: 像素的 Lab 颜色
///
/// # Return:
///   既不近黑/近白、也不近灰则 `true`。
fn is_vivid(lab: &Lab, k: &mineral_config::KmeansConfig) -> bool {
    let chroma = (lab.a * lab.a + lab.b * lab.b).sqrt();
    lab.l >= *k.l_min() && lab.l <= *k.l_max() && chroma >= *k.chroma_min()
}

/// 把一个 Lab 簇心转回 sRGB [`Rgb`]。`palette` 内部做 gamma 编码 + clamp,
/// 故不触 `as_conversions` lint。
///
/// # Params:
///   - `lab`: 簇心的 Lab 颜色
///
/// # Return:
///   sRGB 颜色。
fn lab_to_rgb(lab: Lab) -> Rgb {
    let srgb: Srgb<u8> = Srgb::from_color(lab).into_format();
    Rgb::new(srgb.red, srgb.green, srgb.blue)
}

#[cfg(test)]
mod tests {
    use super::extract_palette;
    use image::{DynamicImage, RgbImage};

    fn kcfg() -> color_eyre::Result<mineral_config::KmeansConfig> {
        Ok(mineral_config::Config::defaults()?
            .tui()
            .cover()
            .kmeans()
            .clone())
    }

    #[test]
    fn empty_image_returns_none() -> color_eyre::Result<()> {
        let img = DynamicImage::ImageRgb8(RgbImage::new(0, 0));
        assert!(extract_palette(&img, &kcfg()?).is_none());
        Ok(())
    }
}
