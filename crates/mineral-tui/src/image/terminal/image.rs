//! 图片引擎缓存的终端图片成品抽象。

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::halfblocks::HalfblocksImage;
use super::iterm2::Iterm2Image;
use super::sixel::SixelImage;
use crate::image::graphics::{GraphicsProtocol, TerminalGraphics};
use crate::image::key::PixelSize;
use crate::image::kitty::KittyImage;

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
    ///   - `source`: 已解码原图
    ///   - `pixels`: preview 对应的目标像素尺寸
    ///   - `cells`: preview 对应的目标 cell 宽高
    ///
    /// # Return:
    ///   halfblock preview 与 RGB 像素缓冲字节数
    pub(crate) fn halfblock_preview(
        source: &DynamicImage,
        pixels: PixelSize,
        cells: (u16, u16),
    ) -> (Self, u64) {
        let preview = HalfblocksImage::encode(source, pixels, cells);
        let bytes = preview.resident_bytes();
        (Self::Halfblocks(preview), bytes)
    }

    /// 按当前 terminal backend 编码一张图片。
    ///
    /// # Params:
    ///   - `source`: 已解码原图
    ///   - `pixels`: rasterized 协议的目标像素尺寸；Kitty 为 `None`
    ///   - `cells`: 目标 cell 宽高
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
        graphics: &TerminalGraphics,
    ) -> color_eyre::Result<Self> {
        match graphics.protocol() {
            GraphicsProtocol::Kitty => Ok(Self::Kitty(KittyImage::encode(
                source,
                graphics.allocate_kitty_image_id(),
                graphics.relay(),
            )?)),
            GraphicsProtocol::Sixel => {
                let pixels = raster_pixels(pixels)?;
                Ok(Self::Sixel(SixelImage::encode(
                    source,
                    pixels,
                    graphics.relay(),
                )?))
            }
            GraphicsProtocol::Iterm2 => {
                let pixels = raster_pixels(pixels)?;
                Ok(Self::Iterm2(Iterm2Image::encode(
                    source,
                    pixels,
                    cells,
                    graphics.relay(),
                )?))
            }
            GraphicsProtocol::Halfblocks => {
                let pixels = raster_pixels(pixels)?;
                Ok(Self::Halfblocks(HalfblocksImage::encode(
                    source, pixels, cells,
                )))
            }
        }
    }

    /// 把成品 place 到 ratatui buffer。
    pub(crate) fn render(&mut self, area: Rect, buffer: &mut Buffer) {
        match self {
            Self::Kitty(image) => image.render(area, buffer),
            Self::Sixel(image) => image.render(area, buffer),
            Self::Iterm2(image) => image.render(area, buffer),
            Self::Halfblocks(image) => image.render(area, buffer),
        }
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
        ))
    }
}

/// 返回非 Kitty 协议必须携带的目标像素尺寸。
fn raster_pixels(pixels: Option<PixelSize>) -> color_eyre::Result<PixelSize> {
    pixels.ok_or_else(|| color_eyre::eyre::eyre!("rasterized terminal image requires pixel size"))
}
