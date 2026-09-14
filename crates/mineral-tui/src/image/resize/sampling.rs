//! 使用 CPU SIMD 采样封面，统一精确尺寸、等比适配、居中裁剪与透明补边。

use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, Rgba, RgbaImage};

use crate::image::key::PixelSize;

/// 显示图保留细节，缩略图以颜色和构图为主；同时决定加速算法与回退算法。
#[derive(Clone, Copy, Debug)]
enum SamplingFilter {
    /// 双线性采样，对应 image 的 Triangle 核。
    Display,

    /// Box 区域采样；整数回退到缩略图算法，浮点回退到 Triangle。
    Thumbnail,
}

/// 按 Triangle 核采样到精确尺寸；调用方负责目标区域的宽高比例。
///
/// # Params:
///   - `source`: 已解码图片
///   - `width`: 输出像素宽度
///   - `height`: 输出像素高度
pub(in crate::image) fn resize_exact(
    source: &DynamicImage,
    width: u32,
    height: u32,
) -> DynamicImage {
    sample(source, width, height, SamplingFilter::Display)
}

/// 按 Box 核采样到精确尺寸，供低清预览和取色使用。
///
/// # Params:
///   - `source`: 已解码图片
///   - `width`: 输出像素宽度
///   - `height`: 输出像素高度
pub(in crate::image) fn thumbnail_exact(
    source: &DynamicImage,
    width: u32,
    height: u32,
) -> DynamicImage {
    sample(source, width, height, SamplingFilter::Thumbnail)
}

/// 等比采样到目标边界内；不补边，目标比源图大时允许放大。
///
/// # Params:
///   - `source`: 已解码图片
///   - `width`: 允许的最大像素宽度
///   - `height`: 允许的最大像素高度
pub(in crate::image) fn thumbnail(source: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    let (width, height) = proportional_dimensions(source, width, height, false);
    thumbnail_exact(source, width, height)
}

/// 先按比例覆盖目标区域，再居中裁剪；奇数留边时多余像素仍位于右侧或下侧。
///
/// # Params:
///   - `source`: 已解码的拼图成员
///   - `width`: 拼图块的像素宽度
///   - `height`: 拼图块的像素高度
pub(in crate::image) fn resize_to_fill(
    source: &DynamicImage,
    width: u32,
    height: u32,
) -> DynamicImage {
    let (scaled_width, scaled_height) = proportional_dimensions(source, width, height, true);
    let scaled = resize_exact(source, scaled_width, scaled_height);
    let left = scaled_width.saturating_sub(width) / 2;
    let top = scaled_height.saturating_sub(height) / 2;
    scaled.crop_imm(left, top, width, height)
}

/// 等比适配到真实终端像素尺寸，左上对齐并补透明边；供 Sixel 和 iTerm2 编码使用。
///
/// # Params:
///   - `source`: 已解码图片
///   - `pixels`: 包含留白的完整输出画布尺寸
pub(in crate::image) fn scale_to_pixels(source: &DynamicImage, pixels: PixelSize) -> RgbaImage {
    let (width, height) = proportional_dimensions(source, pixels.width(), pixels.height(), false);
    let resized = resize_exact(source, width, height).into_rgba8();
    let mut canvas = RgbaImage::from_pixel(pixels.width(), pixels.height(), Rgba([0, 0, 0, 0]));
    image::imageops::overlay(&mut canvas, &resized, 0, 0);
    canvas
}

/// 保留源像素格式、色彩标记和未经 alpha 预乘的采样语义，不额外转换完整像素缓冲。
///
/// 加速采样失败时记录 warn 日志，丢弃未完成的输出，并用 image 的原有算法重新采样。
///
/// # Params:
///   - `source`: 借用原生像素与色彩标记的图片
///   - `width`: 输出像素宽度
///   - `height`: 输出像素高度
///   - `filter`: 双线性显示采样或 Box 预览采样
fn sample(source: &DynamicImage, width: u32, height: u32, filter: SamplingFilter) -> DynamicImage {
    if source.width() == width && source.height() == height {
        return source.clone();
    }
    let mut output = DynamicImage::new(width, height, source.color());
    let fast_filter = match filter {
        SamplingFilter::Display => FilterType::Bilinear,
        SamplingFilter::Thumbnail => FilterType::Box,
    };
    let options = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(fast_filter))
        .use_alpha(false);

    let mut fast_resizer = Resizer::new();
    if let Err(error) = fast_resizer.resize(source, &mut output, &options) {
        mineral_log::warn!(target: "cover", %error,
            source_width = source.width(), source_height = source.height(),
            width, height, pixel_format = ?source.color(), ?filter,
            cpu = ?fast_resizer.cpu_extensions(), "cover pixel sampling failed; falling back to image");

        drop(output);

        output = match (filter, source.color()) {
            // image 的整数缩略图算法对浮点通道也加舍入偏移，会抬高原有亮度。
            (SamplingFilter::Thumbnail, image::ColorType::Rgb32F | image::ColorType::Rgba32F)
            | (SamplingFilter::Display, _) => {
                source.resize_exact(width, height, image::imageops::FilterType::Triangle)
            }
            (SamplingFilter::Thumbnail, _) => source.thumbnail_exact(width, height),
        };
    }
    let color_space = source.color_space();
    output.set_rgb_primaries(color_space.primaries);
    output.set_transfer_function(color_space.transfer);
    output
}

/// 按限制轴计算等比尺寸并四舍五入，保留原有适配／裁剪的像素边界。
///
/// # Params:
///   - `source`: 提供原始宽高比例的图片
///   - `width`: 目标区域像素宽度
///   - `height`: 目标区域像素高度
///   - `fill`: 为真时覆盖目标后裁剪，否则完整放入目标边界
fn proportional_dimensions(
    source: &DynamicImage,
    width: u32,
    height: u32,
    fill: bool,
) -> (u32, u32) {
    let source_width = u64::from(source.width());
    let source_height = u64::from(source.height());
    if source_width == 0 || source_height == 0 {
        return (
            if source_width == 0 { 1 } else { width.max(1) },
            if source_height == 0 { 1 } else { height.max(1) },
        );
    }
    let width_limits = u64::from(width) * source_height <= u64::from(height) * source_width;
    let (numerator, denominator) = if width_limits != fill {
        (u64::from(width), source_width)
    } else {
        (u64::from(height), source_height)
    };
    let rounded =
        |value, numerator, denominator| (value * numerator + denominator / 2) / denominator;
    let scaled_width = rounded(source_width, numerator, denominator).max(1);
    let scaled_height = rounded(source_height, numerator, denominator).max(1);
    let maximum = u64::from(u32::MAX);
    let (scaled_width, scaled_height) = if scaled_width > maximum {
        (
            maximum,
            rounded(source_height, maximum, source_width).max(1),
        )
    } else if scaled_height > maximum {
        (
            rounded(source_width, maximum, source_height).max(1),
            maximum,
        )
    } else {
        (scaled_width, scaled_height)
    };
    (
        u32::try_from(scaled_width).unwrap_or(u32::MAX),
        u32::try_from(scaled_height).unwrap_or(u32::MAX),
    )
}
