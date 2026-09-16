//! 使用上下半块字符绘制缓存的低分辨率 RGBA 图片。

use std::borrow::Borrow;

use image::{DynamicImage, Rgba, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::image::geometry::fitted_area;
use crate::image::key::PixelSize;
use crate::image::resize::{fitted_pixels, resize_transparent, thumbnail, thumbnail_exact};
use crate::render::color::lerp_color;

/// 一张按目标 cell 网格编码的 halfblocks 图片。
pub(crate) struct HalfblocksImage {
    /// 每个 cell 对应上下两个像素；alpha 保留图片透明度和 cell 取整留白。
    pixels: RgbaImage,
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
        let mut pixels = RgbaImage::new(width, height);
        if cells.0 == 0 || cells.1 == 0 {
            return Self { pixels };
        }
        let cell_pixels = (
            u16::try_from(target_pixels.width() / u32::from(cells.0)).unwrap_or(u16::MAX),
            u16::try_from(target_pixels.height() / u32::from(cells.1)).unwrap_or(u16::MAX),
        );
        let area = fitted_area(
            Rect::new(0, 0, cells.0, cells.1),
            source.borrow(),
            cell_pixels,
        );
        let fitted_cells = (area.width, area.height);
        let sampled = sample_halfblocks(
            source.borrow(),
            PixelSize::from_cells(fitted_cells, cell_pixels),
            fitted_cells,
        );
        drop(source);
        // 只对图片自身的 cell 外框滤波，避免 preview 的大画布把像素漏到外框之外。
        let fitted = resize_transparent(
            &DynamicImage::ImageRgba8(sampled),
            u32::from(area.width),
            u32::from(area.height) * 2,
        )
        .into_rgba8();
        image::imageops::overlay(
            &mut pixels,
            &fitted,
            i64::from(area.x),
            i64::from(area.y) * 2,
        );
        Self { pixels }
    }

    /// 等比生成无补边的低清像素,供 Kitty 行内封面复用;留白由最终 placement 决定。
    pub(super) fn thumbnail(source: &DynamicImage, pixels: PixelSize) -> Self {
        Self {
            pixels: thumbnail(source, pixels.width(), pixels.height()).into_rgba8(),
        }
    }

    /// 把 halfblock 像素网格写入 ratatui buffer。
    pub(super) fn render(&self, area: Rect, buffer: &mut Buffer) {
        render_pixels(&self.pixels, area, buffer);
    }

    /// 借用已采样的低清像素，供行内 Kitty 封面复用，避免重新解码原图。
    pub(super) fn pixels(&self) -> &RgbaImage {
        &self.pixels
    }

    /// 返回上下半块 RGBA 像素缓冲的常驻字节数。
    pub(super) fn resident_bytes(&self) -> u64 {
        u64::from(self.pixels.width())
            .saturating_mul(u64::from(self.pixels.height()))
            .saturating_mul(4)
    }
}

/// 直接生成两倍 halfblock 网格的采样画布，按真实像素比例保持原图形状并居中。
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

    let cell_pixels = (
        u16::try_from(target_width / u32::from(cells.0)).unwrap_or(u16::MAX),
        u16::try_from(target_height / u32::from(cells.1)).unwrap_or(u16::MAX),
    );
    let area = fitted_area(Rect::new(0, 0, cells.0, cells.1), source, cell_pixels);
    let bounds = PixelSize::from_cells((area.width, area.height), cell_pixels);
    let fitted = fitted_pixels(source, bounds);
    let sample_width = scaled_axis(fitted.width(), width, target_width).min(width);
    let sample_height = scaled_axis(fitted.height(), height, target_height).min(height);
    let sampled = thumbnail_exact(source, sample_width, sample_height).into_rgba8();
    let left = u32::from(area.x) * 2 + (u32::from(area.width) * 2).saturating_sub(sample_width) / 2;
    let top =
        u32::from(area.y) * 4 + (u32::from(area.height) * 4).saturating_sub(sample_height) / 2;
    image::imageops::overlay(&mut canvas, &sampled, i64::from(left), i64::from(top));
    canvas
}

/// 按非零比例缩放一个像素轴，四舍五入并保留至少一个像素。
fn scaled_axis(value: u32, numerator: u32, denominator: u32) -> u32 {
    let rounded = (u64::from(value) * u64::from(numerator) + u64::from(denominator) / 2)
        / u64::from(denominator);
    u32::try_from(rounded).unwrap_or(u32::MAX).max(1)
}

/// 按真实 cell 比例生成透明的 halfblock 网格，供动画与缓存预览共用。
pub(in crate::image) fn sample_pixels(
    source: &DynamicImage,
    cells: (u16, u16),
    cell_pixels: (u16, u16),
) -> RgbaImage {
    HalfblocksImage::encode(source, PixelSize::from_cells(cells, cell_pixels), cells).pixels
}

/// 将透明网格叠到本帧背景；完全留白的 cell 保留原内容。
pub(in crate::image) fn render_pixels(pixels: &RgbaImage, area: Rect, buffer: &mut Buffer) {
    let visible = area.intersection(buffer.area);
    for y in visible.top()..visible.bottom() {
        for x in visible.left()..visible.right() {
            let column = u32::from(x - area.x);
            let row = u32::from(y - area.y) * 2;
            let (Some(top), Some(bottom)) = (
                pixels.get_pixel_checked(column, row),
                pixels.get_pixel_checked(column, row + 1),
            ) else {
                continue;
            };
            let Rgba([_, _, _, top_alpha]) = *top;
            let Rgba([_, _, _, bottom_alpha]) = *bottom;
            if top_alpha == 0 && bottom_alpha == 0 {
                continue;
            }
            if let Some(cell) = buffer.cell_mut((x, y)) {
                let upper_background = if cell.symbol() == "▀" {
                    cell.fg
                } else {
                    cell.bg
                };
                let upper = over_background(*top, upper_background);
                let lower = over_background(*bottom, cell.bg);
                cell.set_char('▀').set_fg(upper).set_bg(lower);
            }
        }
    }
}

/// alpha 使用当前帧颜色合成；透明像素不固化背景，非 RGB 主题沿用颜色工具的离散混合。
fn over_background(pixel: Rgba<u8>, background: Color) -> Color {
    let Rgba([red, green, blue, alpha]) = pixel;
    lerp_color(
        background,
        Color::Rgb(red, green, blue),
        u64::from(alpha),
        255,
    )
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};

    use super::HalfblocksImage;
    use crate::image::key::PixelSize;

    /// 横图、竖图和非方形 cell 保持物理比例，居中留白保留透明度。
    #[test]
    fn keeps_aspect_ratio_and_centered_transparent_padding() -> color_eyre::Result<()> {
        let cells = (16, 8);
        for (source_size, cell_pixels, content_point, padding_point) in [
            ((300, 150), (8, 16), (8, 8), (8, 0)),
            ((150, 300), (8, 16), (8, 8), (0, 8)),
            ((300, 300), (8, 24), (8, 8), (8, 15)),
        ] {
            let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(
                source_size.0,
                source_size.1,
                Rgb([255, 0, 0]),
            ));
            let image =
                HalfblocksImage::encode(source, PixelSize::from_cells(cells, cell_pixels), cells);
            assert_eq!(image.pixels.dimensions(), (16, 16));
            assert_eq!(image.resident_bytes(), 16 * 16 * 4);
            let content = image
                .pixels
                .get_pixel_checked(content_point.0, content_point.1)
                .ok_or_else(|| color_eyre::eyre::eyre!("content point outside halfblock image"))?;
            let padding = image
                .pixels
                .get_pixel_checked(padding_point.0, padding_point.1)
                .ok_or_else(|| color_eyre::eyre::eyre!("padding point outside halfblock image"))?;
            assert_eq!(*content, Rgba([255, 0, 0, 255]));
            assert_eq!(*padding, Rgba([0, 0, 0, 0]));
        }
        Ok(())
    }

    /// 透明图片和 cell 留白随本帧背景变化，半透明像素按 alpha 合成。
    #[test]
    fn transparent_pixels_follow_the_current_background() -> color_eyre::Result<()> {
        use ratatui::{
            buffer::Buffer,
            layout::Rect,
            style::{Color, Style},
        };
        let cells = (16, 8);
        let area = Rect::new(0, 0, cells.0, cells.1);
        let target = PixelSize::from_cells(cells, (8, 16));
        for alpha in [0, 128, 255] {
            let source =
                DynamicImage::ImageRgba8(RgbaImage::from_pixel(128, 128, Rgba([255, 0, 0, alpha])));
            let image = HalfblocksImage::encode(source, target, cells);
            for blue in [40, 160] {
                let mut buffer = Buffer::empty(area);
                buffer.set_style(area, Style::new().bg(Color::Rgb(0, 0, blue)));
                image.render(area, &mut buffer);
                let cell = buffer
                    .cell((8, 4))
                    .ok_or_else(|| color_eyre::eyre::eyre!("missing cell"))?;
                let expected = crate::render::color::lerp_color(
                    Color::Rgb(0, 0, blue),
                    Color::Rgb(255, 0, 0),
                    u64::from(alpha),
                    255,
                );
                assert_eq!(cell.bg, expected);
                if alpha == 0 {
                    assert_eq!(cell.symbol(), " ");
                } else {
                    assert_eq!(cell.fg, expected);
                }
            }
        }
        Ok(())
    }
}
