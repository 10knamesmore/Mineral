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

/// 从一张封面图提取频谱色板(Lab 明度升序的重点色)。
///
/// 流程:缩到 `kmeans.sample_dim` 采样图(取色不需要全分辨率;box filter 确定性 +
/// 固定 seed,取色仍确定,色板数值与全分辨率聚类略有差异、频谱配色视觉无感)→ 像素整块转 Lab →
/// 丢近黑/近白/近灰(有效像素过少则回退不过滤)→ k-means 聚类(固定 seed,确定性)
/// → 按 Lab 明度升序转回 sRGB。
///
/// # Params:
///   - `img`: 已解码(且 worker 已 resize 到配置上限内)的封面图
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
        img.thumbnail(dim, dim).to_rgb8()
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
    use image::{DynamicImage, RgbImage};
    use palette::{IntoColor, Lab, Srgb};

    use super::extract_palette;
    use crate::render::palette::Rgb;

    /// 测试对照值 = default.lua 的 `cover.kmeans.swatches`。
    const COVER_SWATCHES: usize = 6;

    /// defaults 配置的 kmeans 段(= 接线前硬编码常量)。
    fn kcfg() -> color_eyre::Result<mineral_config::KmeansConfig> {
        Ok(mineral_config::Config::defaults()?
            .tui()
            .cover()
            .kmeans()
            .clone())
    }

    /// 造一张横向均分 `bands` 色块的封面图(60×60),供取色测试喂入。
    ///
    /// # Params:
    ///   - `bands`: 从左到右的色块,至少一个
    ///
    /// # Return:
    ///   填好的 `DynamicImage`;`bands` 为空返回 `Err`。
    fn banded_image(bands: &[Rgb]) -> color_eyre::Result<DynamicImage> {
        banded_image_sized(bands, /*dim*/ 60)
    }

    /// 同 [`banded_image`] 但指定边长(方图)。
    fn banded_image_sized(bands: &[Rgb], dim: u32) -> color_eyre::Result<DynamicImage> {
        let n = u32::try_from(bands.len())?;
        if n == 0 {
            return Err(color_eyre::eyre::eyre!("bands 不能为空"));
        }
        let (w, h) = (dim, dim);
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
        let pal =
            extract_palette(&img, &kcfg()?).ok_or_else(|| color_eyre::eyre::eyre!("应取出色板"))?;
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
        let pal = extract_palette(&img, &kcfg()?)
            .ok_or_else(|| color_eyre::eyre::eyre!("黑白封面应走回退给出色板"))?;
        assert!(!pal.swatches().is_empty(), "回退后色板应非空");
        Ok(())
    }

    /// 384² 大图(> 采样边长)走 thumbnail 降采样路径:色板非空 + Lab 明度仍严格升序。
    #[test]
    fn large_image_palette_still_ordered() -> color_eyre::Result<()> {
        let img = banded_image_sized(
            &[
                Rgb::new(20, 20, 120),
                Rgb::new(200, 40, 40),
                Rgb::new(180, 220, 60),
            ],
            /*dim*/ 384,
        )?;
        let pal = extract_palette(&img, &kcfg()?)
            .ok_or_else(|| color_eyre::eyre::eyre!("大图应取出色板"))?;
        let sw = pal.swatches();
        assert!(sw.len() >= 2, "应聚出 ≥2 色,实际 {}", sw.len());
        let mut prev = f32::MIN;
        for c in sw {
            let lab: Lab = Srgb::new(c.r, c.g, c.b).into_format::<f32>().into_color();
            assert!(lab.l > prev, "明度应严格升序:{} 不大于 {prev}", lab.l);
            prev = lab.l;
        }
        Ok(())
    }

    /// 取色确定性:同一张大图调两次,色板逐字节相等(thumbnail 是确定性 box filter,
    /// k-means seed 固定)。频谱「过渡完就静止」依赖这一点。
    #[test]
    fn palette_is_deterministic() -> color_eyre::Result<()> {
        let img = banded_image_sized(
            &[Rgb::new(20, 20, 120), Rgb::new(200, 40, 40)],
            /*dim*/ 384,
        )?;
        let k = kcfg()?;
        let a =
            extract_palette(&img, &k).ok_or_else(|| color_eyre::eyre::eyre!("第一次应取出色板"))?;
        let b =
            extract_palette(&img, &k).ok_or_else(|| color_eyre::eyre::eyre!("第二次应取出色板"))?;
        assert_eq!(a.swatches(), b.swatches(), "两次取色应逐 swatch 相等");
        Ok(())
    }

    /// 尺寸为 0 的图返回 `None`,不 panic。
    #[test]
    fn empty_image_returns_none() -> color_eyre::Result<()> {
        let img = DynamicImage::ImageRgb8(RgbImage::new(0, 0));
        assert!(extract_palette(&img, &kcfg()?).is_none());
        Ok(())
    }
}
