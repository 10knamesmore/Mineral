//! 按固定像素目标解码封面；JPEG 保留满足两轴需求的最小 IDCT 档位。

use std::io::Cursor;

use color_eyre::eyre::{WrapErr, bail, eyre};
use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};
use jpeg_decoder::{CodingProcess, Decoder, ImageInfo, PixelFormat};

use super::key::PixelSize;
use super::resize::thumbnail_exact;

/// 按配置准备显示像素，保持比例且不放大；无缩小解码能力的格式先完整解码再缩小。
///
/// # Params:
///   - `bytes`: 原始压缩图片
///   - `target`: 用户配置的像素宽高
///
/// # Return:
///   达到目标或原始分辨率的图片；无效图片返回解码错误
pub(super) fn display(
    bytes: &[u8],
    target: &mineral_config::CoverDecodePixelsConfig,
) -> color_eyre::Result<DynamicImage> {
    let required = PixelSize::new(target.width().get(), target.height().get());
    if image::guess_format(bytes)? == ImageFormat::Jpeg {
        let (decoder, info) = open_jpeg(bytes)?;
        if supports_scaling(info) {
            return decode_jpeg(decoder, info, required);
        }
    }
    let image = image::load_from_memory(bytes).wrap_err("decode cover")?;
    let (width, height) = (image.width(), image.height());
    if width <= required.width() || height <= required.height() {
        return Ok(image);
    }
    let (width, height) = if u64::from(required.width()) * u64::from(height)
        >= u64::from(required.height()) * u64::from(width)
    {
        (
            required.width(),
            u32::try_from(
                (u64::from(height) * u64::from(required.width())).div_ceil(u64::from(width)),
            )?,
        )
    } else {
        (
            u32::try_from(
                (u64::from(width) * u64::from(required.height())).div_ceil(u64::from(height)),
            )?,
            required.height(),
        )
    };
    Ok(thumbnail_exact(&image, width, height))
}

/// JPEG 使用不低于两倍 halfblock 网格的 IDCT 缩放，其余编码保持原有解码路径。
///
/// # Params:
///   - `bytes`: 原始压缩图片
///   - `cells`: preview 的终端 cell 宽高
///
/// # Return:
///   可继续区域采样的图片；无效图片返回解码错误
pub(super) fn preview(bytes: &[u8], cells: (u16, u16)) -> color_eyre::Result<DynamicImage> {
    if image::guess_format(bytes)? != ImageFormat::Jpeg {
        return image::load_from_memory(bytes).wrap_err("decode preview");
    }
    let (decoder, info) = open_jpeg(bytes)?;
    if !supports_scaling(info) {
        return image::load_from_memory(bytes).wrap_err("decode unscaled JPEG preview");
    }
    decode_jpeg(
        decoder,
        info,
        PixelSize::new(u32::from(cells.0) * 2, u32::from(cells.1) * 4),
    )
}

/// 读取 JPEG 头，不分配完整像素。
fn open_jpeg(bytes: &[u8]) -> color_eyre::Result<(Decoder<Cursor<&[u8]>>, ImageInfo)> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.read_info().wrap_err("read JPEG header")?;
    let info = decoder.info().ok_or_else(|| eyre!("missing JPEG header"))?;
    Ok((decoder, info))
}

/// RGB 和灰度 DCT 图片可直接缩小解码，其余 JPEG 保留通用解码器的色彩处理。
fn supports_scaling(info: ImageInfo) -> bool {
    info.coding_process != CodingProcess::Lossless
        && matches!(info.pixel_format, PixelFormat::L8 | PixelFormat::RGB24)
}

/// 保留满足两轴像素需求的最小 JPEG IDCT 档位，不再生成精确尺寸的滤波缓冲。
fn decode_jpeg(
    mut decoder: Decoder<Cursor<&[u8]>>,
    info: ImageInfo,
    required: PixelSize,
) -> color_eyre::Result<DynamicImage> {
    let scaled = scaled_dimensions((info.width, info.height), required);
    // scale 在任意一轴满足请求时停止；短轴为 1 时多个倍率取整相同，至少请求 2 让另一轴决定倍率。
    let requested = (scaled.0.max(2), scaled.1.max(2));
    let (width, height) = decoder
        .scale(requested.0, requested.1)
        .wrap_err("scale JPEG cover")?;
    let pixels = decoder.decode().wrap_err("decode scaled JPEG cover")?;
    mineral_log::debug!(target: "cover", source_width = info.width, source_height = info.height,
        decoded_width = width, decoded_height = height, decoded_bytes = pixels.len(),
        "JPEG cover decoded");
    let width = u32::from(width);
    let height = u32::from(height);
    match info.pixel_format {
        PixelFormat::L8 => GrayImage::from_raw(width, height, pixels).map(DynamicImage::ImageLuma8),
        PixelFormat::RGB24 => {
            RgbImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgb8)
        }
        PixelFormat::L16 | PixelFormat::CMYK32 => bail!("scaled JPEG requires RGB or grayscale"),
    }
    .ok_or_else(|| eyre!("JPEG pixels do not match decoded dimensions"))
}

/// 选择两轴均满足采样网格的最大 JPEG 缩小倍率；原图较小时保留原尺寸。
fn scaled_dimensions(source: (u16, u16), required: PixelSize) -> (u16, u16) {
    let required_width = required.width().min(u32::from(source.0));
    let required_height = required.height().min(u32::from(source.1));
    for divisor in [8, 4, 2] {
        let width = source.0.div_ceil(divisor);
        let height = source.1.div_ceil(divisor);
        if u32::from(width) >= required_width && u32::from(height) >= required_height {
            return (width, height);
        }
    }
    source
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{
        DynamicImage, GenericImageView, GrayImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage,
    };

    use super::preview;

    /// JPEG 的 RGB、灰度、奇数边长和极窄图片按采样需求缩小，小图不放大。
    #[test]
    fn jpeg_decode_preserves_sampling_resolution() -> color_eyre::Result<()> {
        for (size, expected) in [
            ((1025, 769), (129, 97)),
            ((300, 300), (150, 150)),
            ((64, 32), (64, 32)),
            ((1, 300), (1, 150)),
        ] {
            let rgb = RgbImage::from_pixel(size.0, size.1, Rgb([180, 80, 40]));
            let gray = GrayImage::from_pixel(size.0, size.1, image::Luma([120]));
            for source in [DynamicImage::ImageRgb8(rgb), DynamicImage::ImageLuma8(gray)] {
                let mut bytes = Cursor::new(Vec::new());
                source.write_to(&mut bytes, ImageFormat::Jpeg)?;
                let decoded = preview(bytes.get_ref(), (48, 24))?;
                assert_eq!(decoded.dimensions(), expected);
                let original = image::load_from_memory(bytes.get_ref())?.to_rgb8();
                let reduced = decoded.to_rgb8();
                let reference = original
                    .get_pixel_checked(/*x*/ 0, /*y*/ 0)
                    .ok_or_else(|| color_eyre::eyre::eyre!("missing reference pixel"))?;
                assert!(reduced.pixels().all(|pixel| {
                    pixel
                        .0
                        .iter()
                        .zip(reference.0)
                        .all(|(actual, expected)| actual.abs_diff(expected) <= 2)
                }));
            }
        }
        Ok(())
    }

    /// PNG 保留完整像素和 alpha，无效压缩输入必须报告错误。
    #[test]
    fn preserves_png_and_rejects_invalid_input() -> color_eyre::Result<()> {
        let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            /*width*/ 301,
            /*height*/ 201,
            Rgba([90, 120, 180, 64]),
        ));
        let mut bytes = Cursor::new(Vec::new());
        source.write_to(&mut bytes, ImageFormat::Png)?;
        assert_eq!(preview(bytes.get_ref(), (48, 24))?, source);
        assert!(preview(b"invalid", (48, 24)).is_err());
        assert!(preview(&[0xff, 0xd8, 0xff, 0xe0], (48, 24)).is_err());
        Ok(())
    }

    /// 固定宽高选择最低 JPEG 档位，不做精确尺寸缩放，小图保留原尺寸。
    #[test]
    fn display_uses_configured_jpeg_dimensions() -> color_eyre::Result<()> {
        use crate::image::key::PixelSize;
        let cfg = mineral_config::Config::defaults()?;
        for (size, expected) in [((2050, 2050), (1025, 1025)), ((300, 200), (300, 200))] {
            let source = DynamicImage::ImageRgb8(RgbImage::new(size.0, size.1));
            let mut bytes = Cursor::new(Vec::new());
            source.write_to(&mut bytes, ImageFormat::Jpeg)?;
            let decoded = super::display(bytes.get_ref(), cfg.tui().cover().decode_pixels())?;
            assert_eq!(decoded.dimensions(), expected);
        }
        assert_eq!(
            super::scaled_dimensions(
                (8300, 8300),
                PixelSize::new(/*width*/ 1024, /*height*/ 1024)
            ),
            (1038, 1038)
        );
        assert_eq!(
            super::scaled_dimensions((4000, 2000), PixelSize::new(/*width*/ 800, /*height*/ 300)),
            (1000, 500)
        );
        Ok(())
    }

    /// 非 JPEG 保持比例和透明度，两轴覆盖配置目标，坏字节返回错误。
    #[test]
    fn display_resizes_png_without_losing_alpha() -> color_eyre::Result<()> {
        let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            /*width*/ 301,
            /*height*/ 201,
            Rgba([90, 120, 180, 64]),
        ));
        let mut bytes = Cursor::new(Vec::new());
        source.write_to(&mut bytes, ImageFormat::Png)?;
        let target = mineral_config::CoverDecodePixelsConfig::builder()
            .width(
                std::num::NonZeroU32::new(/*n*/ 96)
                    .ok_or_else(|| color_eyre::eyre::eyre!("positive width"))?,
            )
            .height(
                std::num::NonZeroU32::new(/*n*/ 80)
                    .ok_or_else(|| color_eyre::eyre::eyre!("positive height"))?,
            )
            .build();
        let decoded = super::display(bytes.get_ref(), &target)?;
        assert_eq!(decoded.dimensions(), (120, 80));
        assert!(
            decoded
                .to_rgba8()
                .pixels()
                .all(|pixel| pixel.0 == [90, 120, 180, 64])
        );
        assert!(super::display(b"not an image", &target).is_err());
        Ok(())
    }
}
