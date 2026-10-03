//! 图片引擎缓存的终端图片成品抽象。

use image::DynamicImage;
use mineral_config::CoverCellFit;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::halfblocks::HalfblocksImage;
use super::iterm2::Iterm2Image;
use super::sixel::SixelImage;
use crate::image::graphics::{GraphicsProtocol, TerminalGraphics};
use crate::image::key::PixelSize;
use crate::image::kitty::KittyImage;
use crate::image::resize::{scale_to_pixels, thumbnail};

/// 终端协议图片编码失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// Kitty 共享内存无法建立或填充。
    #[error("prepare Kitty image")]
    Kitty(#[from] super::super::kitty::shared_memory::Error),

    /// Sixel 图像无法编码。
    #[error("prepare Sixel image")]
    Sixel(#[from] super::sixel::Error),

    /// iTerm2 PNG 无法编码。
    #[error("prepare iTerm2 image")]
    Iterm2(#[from] image::ImageError),

    /// 光栅图缺少目标像素尺寸。
    #[error("rasterized terminal image requires pixel size")]
    MissingPixelSize,
}

/// 一张可 place 到终端的已编码图片。
pub(crate) enum TerminalImage {
    /// Kitty graphics protocol 成品。
    Kitty(KittyImage),

    /// Sixel graphics protocol 成品。
    Sixel(SixelImage),

    /// iTerm2 inline images protocol 成品。
    Iterm2(Iterm2Image),

    /// cell halfblocks 成品。
    Halfblocks(HalfblocksImage),
}

impl TerminalImage {
    /// 生成与终端协议无关的低清 halfblock preview 及其常驻字节数。
    ///
    /// # Params:
    ///   - `source`: 已解码原图，区域采样完成后释放完整像素缓冲
    ///   - `pixels`: preview 对应的目标像素尺寸
    ///   - `cells`: preview 对应的目标 cell 宽高
    ///   - `cell_fit`: 该 preview 缓存键对应的格边适配方式
    ///
    /// # Return:
    ///   halfblock preview 与 RGBA 像素缓冲字节数
    pub(crate) fn halfblock_preview(
        source: DynamicImage,
        pixels: PixelSize,
        cells: (u16, u16),
        cell_fit: CoverCellFit,
    ) -> (Self, u64) {
        let preview = HalfblocksImage::encode(source, pixels, cells, cell_fit);
        let bytes = preview.resident_bytes();
        (Self::Halfblocks(preview), bytes)
    }

    /// 为行内封面采样无补边的等比小图,复用 preview 的 RGBA 存储与预算。
    pub(crate) fn thumbnail_preview(source: &DynamicImage, pixels: PixelSize) -> (Self, u64) {
        let preview = HalfblocksImage::thumbnail(source, pixels);
        let bytes = preview.resident_bytes();
        (Self::Halfblocks(preview), bytes)
    }

    /// 按当前 terminal backend 编码一张图片。
    ///
    /// # Params:
    ///   - `source`: 已解码原图
    ///   - `pixels`: 目标像素尺寸；保持原样的 Kitty 主图为 `None`，行内封面为低清采样上限
    ///   - `cells`: 目标 cell 宽高
    ///   - `cell_fit`: 主封面的格边适配；源图片和行内封面使用 `Contain`
    ///   - `graphics`: 当前 terminal backend 状态
    ///
    /// # Return:
    ///   与当前 backend 协议对应的终端图片成品
    ///
    /// # Error:
    ///   shared memory、Sixel 或 PNG 编码失败时返回错误
    pub(crate) fn encode(
        source: &DynamicImage,
        pixels: Option<PixelSize>,
        cells: (u16, u16),
        cell_fit: CoverCellFit,
        graphics: &TerminalGraphics,
    ) -> Result<Self, Error> {
        match graphics.protocol() {
            GraphicsProtocol::Kitty => {
                // virtual placement 会保持像素源的比例；截边和拉伸必须先生成与 cell 外框
                // 同比例的像素源，不能只改变 placement 的行列数。
                let fitted = pixels.map(|size| match cell_fit {
                    CoverCellFit::Crop | CoverCellFit::Stretch => {
                        DynamicImage::ImageRgba8(scale_to_pixels(source, size, cell_fit))
                    }
                    _ => thumbnail(source, size.width(), size.height()),
                });
                Ok(Self::Kitty(KittyImage::encode(
                    fitted.as_ref().unwrap_or(source),
                    graphics.allocate_kitty_image_id(),
                    graphics.relay(),
                )?))
            }
            GraphicsProtocol::Sixel => {
                let pixels = raster_pixels(pixels)?;
                Ok(Self::Sixel(SixelImage::encode(
                    source,
                    pixels,
                    cell_fit,
                    graphics.relay(),
                )?))
            }
            GraphicsProtocol::Iterm2 => {
                let pixels = raster_pixels(pixels)?;
                Ok(Self::Iterm2(Iterm2Image::encode(
                    source,
                    pixels,
                    cells,
                    cell_fit,
                    graphics.relay(),
                )?))
            }
            GraphicsProtocol::Halfblocks => {
                let pixels = raster_pixels(pixels)?;
                Ok(Self::Halfblocks(HalfblocksImage::encode(
                    source, pixels, cells, cell_fit,
                )))
            }
        }
    }

    /// 复制已采样的低清像素供行内封面编码，其他协议成品不提供像素。
    pub(crate) fn halfblock_source(&self) -> Option<DynamicImage> {
        match self {
            Self::Halfblocks(image) => Some(DynamicImage::ImageRgba8(image.pixels().clone())),
            _ => None,
        }
    }

    /// 写入一个纯 Unicode Kitty 占位 cell，返回须在 cell 输出前发送的图片指令。
    pub(crate) fn render_inline(&mut self, area: Rect, buffer: &mut Buffer) -> Option<String> {
        match self {
            Self::Kitty(image) => image.render_inline(area, buffer),
            _ => None,
        }
    }

    /// 把成品写入 buffer；返回须在 cell 出帧前发送的 Kitty 指令。
    pub(crate) fn render(&mut self, area: Rect, buffer: &mut Buffer) -> Option<String> {
        match self {
            Self::Kitty(image) => return image.render_inline(area, buffer),
            Self::Sixel(image) => image.render(area, buffer),
            Self::Iterm2(image) => image.render(area, buffer),
            Self::Halfblocks(image) => image.render(area, buffer),
        }
        None
    }

    /// 返回该成品持有的像素或协议 payload 字节数；解码原图由独立缓存记账。
    pub(crate) fn resident_bytes(&self) -> u64 {
        match self {
            Self::Kitty(image) => image.resident_bytes(),
            Self::Sixel(image) => image.resident_bytes(),
            Self::Iterm2(image) => image.resident_bytes(),
            Self::Halfblocks(image) => image.resident_bytes(),
        }
    }

    /// 构造缓存测试使用的最小 halfblocks 成品。
    #[cfg(test)]
    pub(crate) fn test_halfblocks() -> Self {
        let source = DynamicImage::ImageRgb8(image::RgbImage::new(1, 2));
        Self::Halfblocks(HalfblocksImage::encode(
            &source,
            PixelSize::from_cells((1, 1), (1, 2)),
            (1, 1),
            CoverCellFit::Contain,
        ))
    }
}

/// 返回非 Kitty 协议必须携带的目标像素尺寸。
fn raster_pixels(pixels: Option<PixelSize>) -> Result<PixelSize, Error> {
    pixels.ok_or(Error::MissingPixelSize)
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, Rgb, RgbImage};
    use mineral_config::CoverCellFit;
    use ratatui::{buffer::Buffer, layout::Rect};

    use super::TerminalImage;
    use crate::image::{graphics::TerminalGraphics, key::PixelSize};

    /// Kitty 截边／拉伸上传目标像素，原样模式上传原图；重复 placement 不重传。
    #[test]
    fn kitty_cell_fit_transmits_the_requested_pixel_dimensions() -> color_eyre::Result<()> {
        let graphics = TerminalGraphics::fixed_kitty((8, 16));
        let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(301, 199, Rgb([180, 80, 40])));
        let area = Rect::new(0, 0, 13, 4);
        let pixels = PixelSize::from_cells((area.width, area.height), graphics.cell_pixels());
        for (cell_fit, target, expected) in [
            (
                CoverCellFit::Crop,
                Some(pixels),
                (pixels.width(), pixels.height()),
            ),
            (
                CoverCellFit::Stretch,
                Some(pixels),
                (pixels.width(), pixels.height()),
            ),
            (
                CoverCellFit::Contain,
                None,
                (source.width(), source.height()),
            ),
        ] {
            let mut image = TerminalImage::encode(
                &source,
                target,
                (area.width, area.height),
                cell_fit,
                &graphics,
            )?;
            let mut buffer = Buffer::empty(area);
            let command = image
                .render(area, &mut buffer)
                .ok_or_else(|| color_eyre::eyre::eyre!("missing Kitty transmission"))?;
            assert!(command.contains(&format!(",s={},v={};", expected.0, expected.1)));
            assert!(image.render(area, &mut buffer).is_none());
        }
        Ok(())
    }
}
