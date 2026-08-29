//! 使用上下半块字符绘制缓存的低分辨率 RGB 图片。

use image::{DynamicImage, Rgb, RgbImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::image::key::PixelSize;
use crate::image::resize::scale_to_pixels;

/// 一张按目标 cell 网格编码的 halfblocks 图片。
pub(crate) struct HalfblocksImage {
    /// 每个 cell 对应上下两个像素的 RGB 图片。
    pixels: RgbImage,
}

impl HalfblocksImage {
    /// 把原图编码成 `width × height*2` 的 halfblock 像素网格。
    ///
    /// # Params:
    ///   - `source`: 已解码原图
    ///   - `target_pixels`: 终端区域的真实像素尺寸
    ///   - `cells`: 目标 cell 宽高
    pub(super) fn encode(
        source: &DynamicImage,
        target_pixels: PixelSize,
        cells: (u16, u16),
    ) -> Self {
        let scaled = DynamicImage::ImageRgba8(scale_to_pixels(source, target_pixels));
        let width = u32::from(cells.0);
        let height = u32::from(cells.1).saturating_mul(2);
        let pixels = scaled
            .resize_exact(width, height, image::imageops::FilterType::Triangle)
            .to_rgb8();
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

/// 返回一个 RGB 像素对应的 ratatui 颜色。
fn color_at(image: &RgbImage, x: u32, y: u32) -> Color {
    image.get_pixel_checked(x, y).map_or(Color::Reset, |pixel| {
        let Rgb([red, green, blue]) = *pixel;
        Color::Rgb(red, green, blue)
    })
}
