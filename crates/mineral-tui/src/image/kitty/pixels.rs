//! 为 Kitty 传输借用 RGB8 / RGBA8 像素，仅转换其他像素格式。

use std::borrow::Cow;

use image::DynamicImage;

/// Kitty 原始像素传输支持的格式。
#[derive(Clone, Copy, Debug)]
pub(super) enum PixelFormat {
    /// 每像素三个 8-bit 分量，不含 alpha。
    Rgb8,

    /// 每像素四个 8-bit 分量，保留 alpha。
    Rgba8,
}

impl PixelFormat {
    /// 返回 Kitty 控制序列 `f` 字段对应的每像素位数。
    pub(super) const fn bits_per_pixel(self) -> u8 {
        match self {
            Self::Rgb8 => 24,
            Self::Rgba8 => 32,
        }
    }
}

/// 可直接写入 shared memory 的像素及其传输格式。
pub(super) struct PixelData<'a> {
    /// 与像素字节布局一致的 Kitty 格式。
    pub(super) format: PixelFormat,

    /// RGB8 / RGBA8 借用原图；其他格式持有转换后的 RGBA8 像素。
    pub(super) bytes: Cow<'a, [u8]>,
}

impl<'a> PixelData<'a> {
    /// 保留原图的 RGB8 / RGBA8 缓冲；仅为其他格式分配 RGBA8 转换缓冲。
    ///
    /// # Params:
    ///   - `source`: 已解码原图，借用像素在传输完成前必须保持有效
    ///
    /// # Return:
    ///   尺寸与原图相同、格式与字节布局匹配的传输像素
    pub(super) fn from_image(source: &'a DynamicImage) -> Self {
        match source {
            DynamicImage::ImageRgb8(image) => Self {
                format: PixelFormat::Rgb8,
                bytes: Cow::Borrowed(image.as_raw()),
            },
            DynamicImage::ImageRgba8(image) => Self {
                format: PixelFormat::Rgba8,
                bytes: Cow::Borrowed(image.as_raw()),
            },
            _ => Self {
                format: PixelFormat::Rgba8,
                bytes: Cow::Owned(source.to_rgba8().into_raw()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, GrayAlphaImage, LumaA, Rgb, RgbImage, Rgba, RgbaImage};

    use super::PixelData;

    /// 原生像素格式必须借用原缓冲，避免为传输额外复制完整封面。
    #[test]
    fn native_pixels_borrow_source_buffer() {
        for source in [
            DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 1, Rgb([10, 20, 30]))),
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 1, Rgba([10, 20, 30, 128]))),
        ] {
            let pixels = PixelData::from_image(&source);
            assert_eq!(pixels.bytes.as_ptr(), source.as_bytes().as_ptr());
            assert_eq!(pixels.bytes.as_ref(), source.as_bytes());
        }
    }

    /// 灰度加 alpha 转换成 Kitty 支持的 RGBA8 时保持亮度与透明度。
    #[test]
    fn converted_pixels_preserve_alpha() {
        let source = DynamicImage::ImageLumaA8(GrayAlphaImage::from_pixel(2, 1, LumaA([42, 64])));
        let pixels = PixelData::from_image(&source);
        assert_eq!(pixels.format.bits_per_pixel(), 32);
        assert_eq!(pixels.bytes.as_ref(), &[42, 42, 42, 64, 42, 42, 42, 64]);
    }
}
