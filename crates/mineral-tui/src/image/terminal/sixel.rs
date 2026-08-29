//! 使用 `icy_sixel` 编码并放置 Sixel 图片。

use color_eyre::eyre::{WrapErr as _, eyre};
use icy_sixel::{
    DiffusionMethod, MethodForLargest, MethodForRep, PixelFormat, Quality, sixel_string,
};
use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_image::CellImage;
use crate::image::graphics::TerminalRelay;
use crate::image::key::PixelSize;
use crate::image::resize::scale_to_pixels;

/// 一张已经编码好的 Sixel 终端图片。
pub(crate) struct SixelImage {
    /// 首 cell 驱动的 Sixel 控制序列。
    image: CellImage,
}

impl SixelImage {
    /// 把原图编码成 Sixel 控制序列。
    ///
    /// # Params:
    ///   - `source`: 已解码原图
    ///   - `pixels`: 目标像素尺寸
    ///   - `relay`: 终端 relay 形态
    ///
    /// # Return:
    ///   已编码 Sixel 成品
    ///
    /// # Error:
    ///   像素尺寸超出编码器范围或 `icy_sixel` 编码失败时返回错误
    pub(super) fn encode(
        source: &DynamicImage,
        pixels: PixelSize,
        relay: TerminalRelay,
    ) -> color_eyre::Result<Self> {
        let image = DynamicImage::ImageRgba8(scale_to_pixels(source, pixels)).to_rgb8();
        let width = i32::try_from(image.width()).wrap_err("convert Sixel width")?;
        let height = i32::try_from(image.height()).wrap_err("convert Sixel height")?;
        let data = sixel_string(
            image.as_raw(),
            width,
            height,
            PixelFormat::RGB888,
            DiffusionMethod::Stucki,
            MethodForLargest::Auto,
            MethodForRep::Auto,
            Quality::HIGH,
        )
        .map_err(|error| eyre!("encode Sixel image: {error}"))?;
        if !data.starts_with('\x1b') {
            color_eyre::eyre::bail!("Sixel encoder returned a sequence without ESC prefix");
        }
        Ok(Self {
            image: CellImage::new(relay.wrap(data)),
        })
    }

    /// 把 Sixel 成品写入 ratatui buffer。
    pub(super) fn render(&self, area: Rect, buffer: &mut Buffer) {
        self.image.render(area, buffer);
    }
}
