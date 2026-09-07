//! 按 halfblock 采样需求解码封面，避免为 JPEG preview 展开完整像素缓冲。

use std::io::Cursor;

use color_eyre::eyre::{WrapErr, bail, eyre};
use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};
use jpeg_decoder::{CodingProcess, Decoder, PixelFormat};

/// JPEG 使用不低于两倍 halfblock 网格的 IDCT 缩放，其余编码保持原有解码路径。
///
/// # Params:
///   - `bytes`: 原始压缩图片
///   - `cells`: preview 的终端 cell 宽高
///
/// # Return:
///   可继续区域采样的图片；无效图片返回解码错误
pub(super) fn decode(bytes: &[u8], cells: (u16, u16)) -> color_eyre::Result<DynamicImage> {
    if image::guess_format(bytes)? != ImageFormat::Jpeg {
        return image::load_from_memory(bytes).wrap_err("decode preview");
    }
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.read_info().wrap_err("read JPEG preview header")?;
    let info = decoder.info().ok_or_else(|| eyre!("missing JPEG header"))?;
    if info.coding_process == CodingProcess::Lossless
        || matches!(info.pixel_format, PixelFormat::L16 | PixelFormat::CMYK32)
    {
        return image::load_from_memory(bytes).wrap_err("decode unscaled JPEG preview");
    }

    let scaled = scaled_dimensions((info.width, info.height), cells);
    // scale 在任意一轴满足请求时停止；短轴为 1 时多个倍率取整相同，至少请求 2 让另一轴决定倍率。
    let requested = (scaled.0.max(2), scaled.1.max(2));
    let (width, height) = decoder
        .scale(requested.0, requested.1)
        .wrap_err("scale JPEG preview")?;
    let pixels = decoder.decode().wrap_err("decode scaled JPEG preview")?;
    mineral_log::debug!(target: "cover", source_width = info.width, source_height = info.height,
        decoded_width = width, decoded_height = height, decoded_bytes = pixels.len(),
        "JPEG preview decoded");
    let width = u32::from(width);
    let height = u32::from(height);
    match info.pixel_format {
        PixelFormat::L8 => GrayImage::from_raw(width, height, pixels).map(DynamicImage::ImageLuma8),
        PixelFormat::RGB24 => {
            RgbImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgb8)
        }
        PixelFormat::L16 | PixelFormat::CMYK32 => bail!("JPEG preview requires RGB or grayscale"),
    }
    .ok_or_else(|| eyre!("JPEG preview pixels do not match decoded dimensions"))
}

/// 选择两轴均满足采样网格的最大 JPEG 缩小倍率；原图较小时保留原尺寸。
fn scaled_dimensions(source: (u16, u16), cells: (u16, u16)) -> (u16, u16) {
    let required_width = (u32::from(cells.0) * 2).min(u32::from(source.0));
    let required_height = (u32::from(cells.1) * 4).min(u32::from(source.1));
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

    use super::decode;

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
                let decoded = decode(bytes.get_ref(), (48, 24))?;
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
        assert_eq!(decode(bytes.get_ref(), (48, 24))?, source);
        assert!(decode(b"invalid", (48, 24)).is_err());
        assert!(decode(&[0xff, 0xd8, 0xff, 0xe0], (48, 24)).is_err());
        Ok(())
    }
}
