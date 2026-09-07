//! 使用上下半块字符绘制缓存的低分辨率 RGB 图片。

use std::borrow::Borrow;

use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::image::key::PixelSize;

/// 一张按目标 cell 网格编码的 halfblocks 图片。
pub(crate) struct HalfblocksImage {
    /// 每个 cell 对应上下两个像素的 RGB 图片。
    pixels: RgbImage,
}

impl HalfblocksImage {
    /// 先区域采样到两倍 halfblock 网格，再滤波到 `width × height*2` 像素。
    ///
    /// # Params:
    ///   - `source`: 已解码原图；转移所有权时在区域采样后立即释放原图
    ///   - `target_pixels`: 终端区域的真实像素尺寸
    ///   - `cells`: 目标 cell 宽高
    pub(super) fn encode(
        source: impl Borrow<DynamicImage>,
        target_pixels: PixelSize,
        cells: (u16, u16),
    ) -> Self {
        let width = u32::from(cells.0);
        let height = u32::from(cells.1).saturating_mul(2);
        let sampled = sample_halfblocks(source.borrow(), target_pixels, cells);
        drop(source);
        let pixels = DynamicImage::ImageRgba8(sampled)
            .resize_exact(width, height, image::imageops::FilterType::Triangle)
            .into_rgb8();
        Self { pixels }
    }

    /// 把 halfblock 像素网格写入 ratatui buffer。
    pub(super) fn render(&self, area: Rect, buffer: &mut Buffer) {
        let width = area
            .width
            .min(u16::try_from(self.pixels.width()).unwrap_or(u16::MAX));
        let pixel_rows = self.pixels.height() / 2;
        let height = area
            .height
            .min(u16::try_from(pixel_rows).unwrap_or(u16::MAX));
        for row in 0..height {
            let upper_row = u32::from(row).saturating_mul(2);
            let lower_row = upper_row.saturating_add(1);
            for column in 0..width {
                let column_px = u32::from(column);
                let style = Style::new()
                    .fg(color_at(&self.pixels, column_px, upper_row))
                    .bg(color_at(&self.pixels, column_px, lower_row));
                buffer.set_string(area.x + column, area.y + row, "▀", style);
            }
        }
    }

    /// 返回上下半块 RGB 像素缓冲的常驻字节数。
    pub(super) fn resident_bytes(&self) -> u64 {
        u64::from(self.pixels.width())
            .saturating_mul(u64::from(self.pixels.height()))
            .saturating_mul(3)
    }
}

/// 直接生成两倍 halfblock 网格的采样画布，按真实像素比例保持原图形状和左上留白布局。
///
/// # Params:
///   - `source`: 已解码原图
///   - `target_pixels`: 显示区域的真实像素尺寸，用于计算原图占据的比例
///   - `cells`: 最终 cell 网格尺寸
///
/// # Return:
///   `cells.width*2 × cells.height*4` 的 RGBA8 画布，未覆盖区域透明
fn sample_halfblocks(
    source: &DynamicImage,
    target_pixels: PixelSize,
    cells: (u16, u16),
) -> RgbaImage {
    let width = u32::from(cells.0) * 2;
    let height = u32::from(cells.1) * 4;
    let mut canvas = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    let target_width = target_pixels.width();
    let target_height = target_pixels.height();
    if width == 0
        || height == 0
        || target_width == 0
        || target_height == 0
        || source.width() == 0
        || source.height() == 0
    {
        return canvas;
    }

    // 先按真实像素等比适配，再映射到 halfblock 网格；每个半块的物理宽高不一定相等。
    let (fitted_width, fitted_height) = if u64::from(source.width()) * u64::from(target_height)
        >= u64::from(source.height()) * u64::from(target_width)
    {
        (
            target_width,
            scaled_axis(source.height(), target_width, source.width()),
        )
    } else {
        (
            scaled_axis(source.width(), target_height, source.height()),
            target_height,
        )
    };
    let sample_width = scaled_axis(fitted_width, width, target_width).min(width);
    let sample_height = scaled_axis(fitted_height, height, target_height).min(height);
    let sampled = source
        .thumbnail_exact(sample_width, sample_height)
        .into_rgba8();
    image::imageops::overlay(&mut canvas, &sampled, 0, 0);
    canvas
}

/// 按非零比例缩放一个像素轴，四舍五入并保留至少一个像素。
fn scaled_axis(value: u32, numerator: u32, denominator: u32) -> u32 {
    let rounded = (u64::from(value) * u64::from(numerator) + u64::from(denominator) / 2)
        / u64::from(denominator);
    u32::try_from(rounded).unwrap_or(u32::MAX).max(1)
}

/// 返回一个 RGB 像素对应的 ratatui 颜色。
fn color_at(image: &RgbImage, x: u32, y: u32) -> Color {
    image.get_pixel_checked(x, y).map_or(Color::Reset, |pixel| {
        let Rgb([red, green, blue]) = *pixel;
        Color::Rgb(red, green, blue)
    })
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};

    use super::HalfblocksImage;
    use crate::image::key::PixelSize;

    /// 横图、竖图和非方形半块保持物理比例，空余区域仍在右侧或下方。
    #[test]
    fn keeps_aspect_ratio_and_top_left_padding() -> color_eyre::Result<()> {
        let cells = (16, 8);
        for (source_size, cell_pixels, content_point, padding_point) in [
            ((300, 150), (8, 16), (4, 3), (4, 12)),
            ((150, 300), (8, 16), (3, 4), (12, 4)),
            ((300, 300), (8, 24), (4, 8), (4, 14)),
        ] {
            let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(
                source_size.0,
                source_size.1,
                Rgb([255, 0, 0]),
            ));
            let image =
                HalfblocksImage::encode(source, PixelSize::from_cells(cells, cell_pixels), cells);
            assert_eq!(image.pixels.dimensions(), (16, 16));
            assert_eq!(image.resident_bytes(), 16 * 16 * 3);
            let content = image
                .pixels
                .get_pixel_checked(content_point.0, content_point.1)
                .ok_or_else(|| color_eyre::eyre::eyre!("content point outside halfblock image"))?;
            let padding = image
                .pixels
                .get_pixel_checked(padding_point.0, padding_point.1)
                .ok_or_else(|| color_eyre::eyre::eyre!("padding point outside halfblock image"))?;
            assert_eq!(*content, Rgb([255, 0, 0]));
            assert_eq!(*padding, Rgb([0, 0, 0]));
        }
        Ok(())
    }

    /// 完全透明像素保持黑色背景，半透明像素保持原有丢弃 alpha 后的 RGB 颜色。
    #[test]
    fn preserves_transparent_pixel_behavior() {
        let cells = (16, 8);
        let target = PixelSize::from_cells(cells, (8, 16));
        for (alpha, expected) in [(0, Rgb([0, 0, 0])), (128, Rgb([255, 0, 0]))] {
            let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                /*width*/ 128,
                /*height*/ 128,
                Rgba([255, 0, 0, alpha]),
            ));
            let image = HalfblocksImage::encode(source, target, cells);
            assert!(image.pixels.pixels().all(|pixel| *pixel == expected));
        }
    }
}
