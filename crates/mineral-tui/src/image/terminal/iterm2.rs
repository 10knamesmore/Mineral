//! 编码并放置 iTerm2 inline image OSC 1337 图片。

use std::fmt::Write as _;
use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_image::CellImage;
use crate::image::graphics::TerminalRelay;
use crate::image::key::PixelSize;
use crate::image::resize::scale_to_pixels;

/// 一张已经编码好的 iTerm2 inline image。
pub(crate) struct Iterm2Image {
    /// 首 cell 驱动的 OSC 1337 控制序列。
    image: CellImage,
}

impl Iterm2Image {
    /// 把原图编码成内联 PNG OSC 1337 控制序列。
    ///
    /// # Params:
    ///   - `source`: 已解码原图
    ///   - `pixels`: 目标像素尺寸
    ///   - `cells`: 目标 cell 宽高
    ///   - `relay`: 终端 relay 形态
    ///
    /// # Return:
    ///   已编码 iTerm2 成品
    ///
    /// # Error:
    ///   PNG 编码失败时返回错误
    pub(super) fn encode(
        source: &DynamicImage,
        pixels: PixelSize,
        cells: (u16, u16),
        relay: TerminalRelay,
    ) -> color_eyre::Result<Self> {
        let scaled = DynamicImage::ImageRgba8(scale_to_pixels(source, pixels));
        let mut cursor = Cursor::new(Vec::<u8>::new());
        scaled.write_to(&mut cursor, image::ImageFormat::Png)?;
        let png = cursor.into_inner();
        let payload = STANDARD.encode(&png);
        let mut sequence = String::new();
        for _ in 0..cells.1 {
            let _ = write!(sequence, "\x1b[{}X\x1b[1B", cells.0);
        }
        let _ = write!(sequence, "\x1b[{}A", cells.1);
        let _ = write!(
            sequence,
            "\x1b]1337;File=inline=1;size={};width={}px;height={}px;doNotMoveCursor=1:{}\x07",
            png.len(),
            pixels.width(),
            pixels.height(),
            payload,
        );
        Ok(Self {
            image: CellImage::new(relay.wrap(sequence)),
        })
    }

    /// 把 iTerm2 成品写入 ratatui buffer。
    pub(super) fn render(&self, area: Rect, buffer: &mut Buffer) {
        self.image.render(area, buffer);
    }

    /// 返回内联 PNG 控制序列的实际分配字节数。
    pub(super) fn resident_bytes(&self) -> u64 {
        self.image.resident_bytes()
    }
}
