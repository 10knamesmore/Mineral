//! 封面取色:把一张已解码的封面图聚成若干重点色,沿 Lab 明度升序排成 [`CoverPalette`]。
//!
//! 跑在 cover fetcher worker 的 `spawn_blocking` 里(CPU 密集,与解码同处),色带随图回传,
//! **不在渲染线程现算**。取色是封面的附属信息,尽力而为:任一步失败返回 `None`,
//! 频谱回退 hue 漂移,封面图本身照常显示。

use image::DynamicImage;
use kmeans_colors::{Sort, get_kmeans};
use palette::cast::from_component_slice;
use palette::{FromColor, IntoColor, Lab, Srgb};

use crate::render::palette::{CoverPalette, Rgb};

/// k-means 簇数 = 取出的重点色上限。6 个色沿频率轴铺开,既有层次又不糊成一片。
const COVER_SWATCHES: usize = 6;

/// k-means 初始中心的随机种子。**固定值是硬要求**:不固定则同封面每次取色结果不同、
/// 颜色会跳,违反"过渡完就静止"。取任意固定整数即可。
const COVER_KMEANS_SEED: u64 = 0x5EED_C0DE;

/// k-means 最大迭代次数(库推荐量级)。封面色块少,20 次足够收敛。
const COVER_KMEANS_MAX_ITER: usize = 20;

/// k-means 收敛阈值。`palette` 文档对 Lab 空间推荐 5.0。
const COVER_KMEANS_CONVERGE: f32 = 5.0;

/// 丢弃近黑像素的 Lab 明度下限(L ∈ 0..=100)。避免纯黑背景霸占色板。
const L_MIN: f32 = 8.0;

/// 丢弃近白像素的 Lab 明度上限。避免纯白背景霸占色板。
const L_MAX: f32 = 92.0;

/// 丢弃近灰像素的 Lab 彩度下限(`√(a²+b²)`)。灰背景对沿频率铺色没有贡献。
const CHROMA_MIN: f32 = 8.0;

/// 过滤后有效像素占比低于此(%)则放弃过滤、改用全部像素。黑白 / 低饱和封面也得有色。
const MIN_VALID_PIXELS_PCT: usize = 5;

/// 从一张封面图提取频谱色板(Lab 明度升序的重点色)。
///
/// 流程:像素整块转 Lab → 丢近黑/近白/近灰(有效像素过少则回退不过滤)→ k-means 聚类
/// (固定 seed,确定性)→ 按 Lab 明度升序转回 sRGB。
///
/// # Params:
///   - `img`: 已解码(且 worker 已 resize 到 ≤ `COVER_MAX_DIM`)的封面图
///
/// # Return:
///   `Some(CoverPalette)`;尺寸为 0 / 有效像素为 0 / 聚类无结果时 `None`。
pub fn extract_palette(img: &DynamicImage) -> Option<CoverPalette> {
    let rgb = img.to_rgb8();
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
    let filtered: Vec<Lab> = all_lab.iter().copied().filter(is_vivid).collect();
    // 有效像素太少(黑白 / 低饱和封面)则不过滤,用全部像素兜底。
    let valid_pct = filtered.len().saturating_mul(100) / all_lab.len();
    let samples: &[Lab] = if valid_pct < MIN_VALID_PIXELS_PCT {
        &all_lab
    } else {
        &filtered
    };
    if samples.is_empty() {
        return None;
    }

    let result = get_kmeans(
        COVER_SWATCHES,
        COVER_KMEANS_MAX_ITER,
        COVER_KMEANS_CONVERGE,
        false, /*verbose*/
        samples,
        COVER_KMEANS_SEED,
    );
    // `sort_indexed_colors` 内置丢弃空簇 + 按 L 暗→亮排序,正好是"低频暗→高频亮"。
    let sorted = Lab::sort_indexed_colors(&result.centroids, &result.indices);
    let swatches: Vec<Rgb> = sorted.iter().map(|cd| lab_to_rgb(cd.centroid)).collect();
    CoverPalette::new(swatches)
}

/// 像素是否"有色":明度在 [`L_MIN`]..=[`L_MAX`] 且彩度 ≥ [`CHROMA_MIN`]。
///
/// # Params:
///   - `lab`: 像素的 Lab 颜色
///
/// # Return:
///   既不近黑/近白、也不近灰则 `true`。
fn is_vivid(lab: &Lab) -> bool {
    let chroma = (lab.a * lab.a + lab.b * lab.b).sqrt();
    lab.l >= L_MIN && lab.l <= L_MAX && chroma >= CHROMA_MIN
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
    use image::{DynamicImage, RgbImage};
    use palette::{IntoColor, Lab, Srgb};

    use super::{COVER_SWATCHES, extract_palette};
    use crate::render::palette::Rgb;

    /// 造一张横向均分 `bands` 色块的封面图(60×60),供取色测试喂入。
    ///
    /// # Params:
    ///   - `bands`: 从左到右的色块,至少一个
    ///
    /// # Return:
    ///   填好的 `DynamicImage`;`bands` 为空返回 `Err`。
    fn banded_image(bands: &[Rgb]) -> color_eyre::Result<DynamicImage> {
        let n = u32::try_from(bands.len())?;
        if n == 0 {
            return Err(color_eyre::eyre::eyre!("bands 不能为空"));
        }
        let (w, h) = (60_u32, 60_u32);
        let mut img = RgbImage::new(w, h);
        for (x, _y, px) in img.enumerate_pixels_mut() {
            let band = (x * n / w).min(n - 1);
            let sw = bands
                .get(usize::try_from(band)?)
                .ok_or_else(|| color_eyre::eyre::eyre!("band 越界"))?;
            *px = image::Rgb([sw.r, sw.g, sw.b]);
        }
        Ok(DynamicImage::ImageRgb8(img))
    }

    /// 三块明度分明的纯色:聚出 2..=k 个色,且 swatches 明度严格升序(暗→亮)。
    #[test]
    fn orders_swatches_by_ascending_lightness() -> color_eyre::Result<()> {
        // 暗蓝(低 L)/ 中红(中 L)/ 亮黄绿(高 L),三者明度拉开、彩度都够。
        let img = banded_image(&[
            Rgb::new(20, 20, 120),
            Rgb::new(200, 40, 40),
            Rgb::new(180, 220, 60),
        ])?;
        let pal = extract_palette(&img).ok_or_else(|| color_eyre::eyre::eyre!("应取出色板"))?;
        let sw = pal.swatches();
        assert!(
            (2..=COVER_SWATCHES).contains(&sw.len()),
            "应聚出 2..={COVER_SWATCHES} 色,实际 {}",
            sw.len()
        );
        let mut prev = f32::MIN;
        for c in sw {
            let lab: Lab = Srgb::new(c.r, c.g, c.b).into_format::<f32>().into_color();
            assert!(lab.l > prev, "明度应严格升序:{} 不大于 {prev}", lab.l);
            prev = lab.l;
        }
        Ok(())
    }

    /// 近纯黑封面:过滤后有效像素为 0,走回退用全部像素,仍给出非空色板(不返回 `None`)。
    #[test]
    fn near_black_falls_back_to_all_pixels() -> color_eyre::Result<()> {
        let img = banded_image(&[Rgb::new(2, 2, 2)])?;
        let pal = extract_palette(&img)
            .ok_or_else(|| color_eyre::eyre::eyre!("黑白封面应走回退给出色板"))?;
        assert!(!pal.swatches().is_empty(), "回退后色板应非空");
        Ok(())
    }

    /// 尺寸为 0 的图返回 `None`,不 panic。
    #[test]
    fn empty_image_returns_none() {
        let img = DynamicImage::ImageRgb8(RgbImage::new(0, 0));
        assert!(extract_palette(&img).is_none());
    }
}
