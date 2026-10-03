//! 使用 `icy_sixel` 编码并放置 Sixel 图片。

use icy_sixel::{
    DiffusionMethod, MethodForLargest, MethodForRep, PixelFormat, Quality, SixelError, sixel_string,
};
use image::DynamicImage;
use mineral_config::CoverCellFit;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_image::CellImage;
use crate::image::graphics::TerminalRelay;
use crate::image::key::PixelSize;
use crate::image::resize::scale_to_pixels;

/// Sixel 编码失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// 编码器整数尺寸超出范围。
    #[error("convert Sixel dimensions")]
    Dimensions(#[from] std::num::TryFromIntError),

    /// Sixel 编码器返回错误。
    #[error("encode Sixel image")]
    Encode(#[source] SixelError),

    /// 编码器返回了其公开错误类型以外的错误。
    #[error("Sixel encoder returned an unrecognized error type")]
    UnexpectedEncoderFailure,

    /// 编码器返回的内容不是 Sixel 控制序列。
    #[error("Sixel encoder returned a sequence without ESC prefix")]
    InvalidSequence,
}

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
    ///   - `pixels`: 最小字符外框的像素尺寸
    ///   - `cell_fit`: 在该外框内截边、拉伸或留白
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
        cell_fit: CoverCellFit,
        relay: TerminalRelay,
    ) -> Result<Self, Error> {
        let image = DynamicImage::ImageRgba8(scale_to_pixels(source, pixels, cell_fit)).to_rgb8();
        let width = i32::try_from(image.width())?;
        let height = i32::try_from(image.height())?;
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
        .map_err(|error| match error.downcast::<SixelError>() {
            Ok(source) => Error::Encode(*source),
            Err(source) => {
                mineral_log::warn!(target: "cover", error = mineral_log::chain(source.as_ref()), "Sixel encoder returned an unexpected error type");
                Error::UnexpectedEncoderFailure
            }
        })?;
        if !data.starts_with('\x1b') {
            return Err(Error::InvalidSequence);
        }
        Ok(Self {
            image: CellImage::new(relay.wrap(data)),
        })
    }

    /// 把 Sixel 成品写入 ratatui buffer。
    pub(super) fn render(&self, area: Rect, buffer: &mut Buffer) {
        self.image.render(area, buffer);
    }

    /// 返回 Sixel 控制序列的实际分配字节数。
    pub(super) fn resident_bytes(&self) -> u64 {
        self.image.resident_bytes()
    }
}
