//! 使用 CPU SIMD 采样封面，保留原生像素格式与色彩标记。

use fast_image_resize::images::CroppedImage;
use fast_image_resize::{CropBox, FilterType, ResizeAlg, ResizeError, ResizeOptions, Resizer};
use image::{DynamicImage, RgbaImage};

use crate::image::key::PixelSize;

/// 图片像素采样失败；调用方决定记录日志或结束当前资源任务。
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("image sampling requires nonzero source and target dimensions")]
    EmptyDimensions,

    #[error("resize image pixels")]
    Resize(#[from] ResizeError),
}

/// 需要生成精确物理像素画布的两种适配策略。
#[derive(Clone, Copy, Debug)]
pub(crate) enum PixelFit {
    Crop,

    Stretch,
}

/// 显示图保留细节，缩略图以颜色和构图为主。
#[derive(Clone, Copy, Debug)]
enum SamplingFilter {
    /// 双线性采样，对应 image 的 Triangle 核。
    Display,

    /// 透明画布使用 alpha 加权滤波，防止补边黑色污染图片边缘。
    Transparent,

    /// Box 区域采样。
    Thumbnail,
}

/// 源图的取样范围；整数像素边界与按目标比例计算的浮点裁剪分别处理。
#[derive(Clone, Copy, Debug)]
enum SamplingRegion {
    Whole,

    Fit(PixelSize),

    Crop {
        left: u32,

        top: u32,

        width: u32,

        height: u32,
    },
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
) -> Result<DynamicImage, Error> {
    sample(
        source,
        width,
        height,
        SamplingFilter::Display,
        SamplingRegion::Whole,
    )
}

/// 借用源图的整数中心方图，按 Triangle 核采样；奇数留边多出的像素位于右侧或下侧。
pub(in crate::image) fn resize_center_square(
    source: &DynamicImage,
    output_side: u32,
) -> Result<DynamicImage, Error> {
    let source_side = source.width().min(source.height());
    sample(
        source,
        output_side,
        output_side,
        SamplingFilter::Display,
        SamplingRegion::Crop {
            left: (source.width() - source_side) / 2,
            top: (source.height() - source_side) / 2,
            width: source_side,
            height: source_side,
        },
    )
}

/// 对带透明补边的画布执行 alpha 加权缩放，保留边缘颜色。
pub(in crate::image) fn resize_transparent(
    source: &DynamicImage,
    width: u32,
    height: u32,
) -> Result<DynamicImage, Error> {
    sample(
        source,
        width,
        height,
        SamplingFilter::Transparent,
        SamplingRegion::Whole,
    )
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
) -> Result<DynamicImage, Error> {
    sample(
        source,
        width,
        height,
        SamplingFilter::Thumbnail,
        SamplingRegion::Whole,
    )
}

/// 按字符外框的物理比例居中截边，再采样到低清网格；网格宽高不参与裁剪比例计算。
///
/// # Params:
///   - `source`: 已解码原图
///   - `bounds`: 最小字符外框的真实像素尺寸
///   - `width` / `height`: 输出采样网格的宽高
pub(in crate::image) fn thumbnail_to_fill(
    source: &DynamicImage,
    bounds: PixelSize,
    width: u32,
    height: u32,
) -> Result<DynamicImage, Error> {
    sample(
        source,
        width,
        height,
        SamplingFilter::Thumbnail,
        SamplingRegion::Fit(bounds),
    )
}

/// 等比采样到目标边界内；不补边，目标比源图大时允许放大。
///
/// # Params:
///   - `source`: 已解码图片
///   - `width`: 允许的最大像素宽度
///   - `height`: 允许的最大像素高度
pub(in crate::image) fn thumbnail(
    source: &DynamicImage,
    width: u32,
    height: u32,
) -> Result<DynamicImage, Error> {
    let (width, height) = proportional_dimensions(source, width, height, false)?;
    thumbnail_exact(source, width, height)
}

/// 计算完整容纳原图的等比像素尺寸；整数像素按四舍五入取整。
pub(in crate::image) fn fitted_pixels(
    source: &DynamicImage,
    bounds: PixelSize,
) -> Result<PixelSize, Error> {
    let (width, height) = proportional_dimensions(source, bounds.width(), bounds.height(), false)?;
    Ok(PixelSize::new(width, height))
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
) -> Result<DynamicImage, Error> {
    let (scaled_width, scaled_height) = proportional_dimensions(source, width, height, true)?;
    let scaled = resize_exact(source, scaled_width, scaled_height)?;
    let left = scaled_width.saturating_sub(width) / 2;
    let top = scaled_height.saturating_sub(height) / 2;
    Ok(scaled.crop_imm(left, top, width, height))
}

/// 按格边策略适配到最小字符外框的物理像素尺寸，供终端图片编码使用。
///
/// # Params:
///   - `source`: 已解码图片
///   - `pixels`: 最小字符外框的完整像素尺寸，不是整个封面槽
///   - `fit`: 居中截边或微调宽高
pub(in crate::image) fn scale_to_pixels(
    source: &DynamicImage,
    pixels: PixelSize,
    fit: PixelFit,
) -> Result<RgbaImage, Error> {
    let sampled = match fit {
        PixelFit::Crop => sample(
            source,
            pixels.width(),
            pixels.height(),
            SamplingFilter::Display,
            SamplingRegion::Fit(pixels),
        )?,
        PixelFit::Stretch => resize_exact(source, pixels.width(), pixels.height())?,
    };
    Ok(sampled.into_rgba8())
}

/// 保留源像素格式和色彩标记；仅透明画布滤波使用 alpha 加权，不额外转换完整像素缓冲。
///
/// # Params:
///   - `source`: 借用原生像素与色彩标记的图片
///   - `width`: 输出像素宽度
///   - `height`: 输出像素高度
///   - `filter`: 双线性显示采样或 Box 预览采样
///   - `region`: 采样全部原图、按目标比例裁剪或取整数像素区域
fn sample(
    source: &DynamicImage,
    width: u32,
    height: u32,
    filter: SamplingFilter,
    region: SamplingRegion,
) -> Result<DynamicImage, Error> {
    validate_dimensions(source, width, height)?;
    if matches!(region, SamplingRegion::Whole)
        && source.width() == width
        && source.height() == height
    {
        return Ok(source.clone());
    }
    let mut output = DynamicImage::new(width, height, source.color());
    let fast_filter = match filter {
        SamplingFilter::Display | SamplingFilter::Transparent => FilterType::Bilinear,
        SamplingFilter::Thumbnail => FilterType::Box,
    };
    let mut options = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(fast_filter))
        .use_alpha(matches!(filter, SamplingFilter::Transparent));
    if let SamplingRegion::Fit(bounds) = region {
        validate_dimensions(source, bounds.width(), bounds.height())?;
        let crop = CropBox::fit_src_into_dst_size(
            source.width(),
            source.height(),
            bounds.width(),
            bounds.height(),
            None,
        );
        options = options.crop(crop.left, crop.top, crop.width, crop.height);
    }

    let mut fast_resizer = Resizer::new();
    let result = match region {
        SamplingRegion::Crop {
            left,
            top,
            width,
            height,
        } => CroppedImage::new(source, left, top, width, height)
            .map_err(ResizeError::from)
            .and_then(|cropped| fast_resizer.resize(&cropped, &mut output, &options)),
        SamplingRegion::Whole | SamplingRegion::Fit(_) => {
            fast_resizer.resize(source, &mut output, &options)
        }
    };
    result?;
    let color_space = source.color_space();
    output.set_rgb_primaries(color_space.primaries);
    output.set_transfer_function(color_space.transfer);
    Ok(output)
}

fn validate_dimensions(source: &DynamicImage, width: u32, height: u32) -> Result<(), Error> {
    if source.width() == 0 || source.height() == 0 || width == 0 || height == 0 {
        Err(Error::EmptyDimensions)
    } else {
        Ok(())
    }
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
) -> Result<(u32, u32), Error> {
    validate_dimensions(source, width, height)?;
    let source_width = u64::from(source.width());
    let source_height = u64::from(source.height());
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
    Ok((
        u32::try_from(scaled_width).unwrap_or(u32::MAX),
        u32::try_from(scaled_height).unwrap_or(u32::MAX),
    ))
}
