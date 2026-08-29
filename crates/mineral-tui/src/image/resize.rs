//! 把原图按终端目标像素尺寸等比缩放并补透明边。

use image::{DynamicImage, Rgba, RgbaImage};

use crate::image::key::PixelSize;

/// 生成与目标像素尺寸完全一致的 RGBA8 图片。
///
/// # Params:
///   - `source`: 已解码原图
///   - `pixels`: 目标像素尺寸
///
/// # Return:
///   左上对齐、透明补边的 RGBA8 图片
pub(crate) fn scale_to_pixels(source: &DynamicImage, pixels: PixelSize) -> RgbaImage {
    let resized = source
        .resize(
            pixels.width(),
            pixels.height(),
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    let mut canvas = RgbaImage::from_pixel(pixels.width(), pixels.height(), Rgba([0, 0, 0, 0]));
    image::imageops::overlay(&mut canvas, &resized, 0, 0);
    canvas
}
