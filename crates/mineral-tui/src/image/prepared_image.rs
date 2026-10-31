//! 主封面与行内封面使用的稳态成品，统一绘制半块像素和 Kitty 占位字符。

use std::sync::Arc;

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::halfblock::HalfblockImage;
use crate::config::CoverCellFit;
use crate::image::graphics::{GraphicsProtocol, TerminalGraphics};
use crate::image::key::TerminalImageKey;
use crate::image::kitty::shared_memory;
use crate::image::kitty::{KittyImage, KittyImageHandle};
use crate::image::resize::{self, PixelFit, scale_to_pixels, thumbnail};

/// 终端图片编码失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// Kitty 共享内存无法建立或填充。
    #[error("prepare Kitty image")]
    Kitty(#[from] shared_memory::Error),

    #[error("sample terminal image pixels")]
    Sampling(#[from] resize::Error),
}

/// 一张可 place 到终端的已编码图片。
pub(crate) enum PreparedImage {
    /// Kitty graphics protocol 成品。
    Kitty(KittyImage),

    /// 可直接绘制到 cell buffer 的半块像素。
    Halfblock(HalfblockImage),
}

impl PreparedImage {
    /// 按本次会话固定的协议编码一张图片。
    ///
    /// # Params:
    ///   - `source`: 已解码原图
    ///   - `key`: 源图片、独立 Kitty 缩略图或带完整采样几何的光栅成品
    ///   - `graphics`: 本次会话的输出能力
    ///
    /// # Return:
    ///   与会话协议对应的终端图片成品
    ///
    /// # Error:
    ///   像素采样或 Kitty 共享内存写入失败时返回错误
    pub(crate) fn encode(
        source: &DynamicImage,
        key: &TerminalImageKey,
        graphics: &TerminalGraphics,
    ) -> Result<Self, Error> {
        let fitted = match key {
            TerminalImageKey::Source(_) => {
                return Ok(Self::Kitty(KittyImage::encode(source, graphics)?));
            }
            TerminalImageKey::Thumbnail { pixels, .. } => {
                thumbnail(source, pixels.width(), pixels.height())?
            }
            TerminalImageKey::Rasterized {
                pixels,
                cells,
                cell_fit,
                ..
            } => {
                if graphics.protocol() == GraphicsProtocol::Halfblocks {
                    return Ok(Self::Halfblock(HalfblockImage::encode(
                        source, *pixels, *cells, *cell_fit,
                    )?));
                }
                // Kitty placement 保持源比例；截边和拉伸必须先改变上传像素。
                match cell_fit {
                    CoverCellFit::Crop => {
                        DynamicImage::ImageRgba8(scale_to_pixels(source, *pixels, PixelFit::Crop)?)
                    }
                    CoverCellFit::Stretch => DynamicImage::ImageRgba8(scale_to_pixels(
                        source,
                        *pixels,
                        PixelFit::Stretch,
                    )?),
                    CoverCellFit::Contain => thumbnail(source, pixels.width(), pixels.height())?,
                }
            }
        };
        Ok(Self::Kitty(KittyImage::encode(&fitted, graphics)?))
    }

    /// 为行内转场保留 Kitty 图片；半块字符成品不提供行内缩略图。
    pub(crate) fn inline_handle(&self) -> Option<Arc<KittyImageHandle>> {
        match self {
            Self::Kitty(image) => Some(image.handle()),
            _ => None,
        }
    }

    /// 写入纯 Unicode Kitty 占位字符，不修改终端协议状态。
    pub(crate) fn render_inline(&self, area: Rect, buffer: &mut Buffer) {
        if let Self::Kitty(image) = self {
            image.render_inline(area, buffer);
        }
    }

    /// 把已编码成品写入目标 buffer。
    pub(crate) fn render(&self, area: Rect, buffer: &mut Buffer) {
        match self {
            Self::Kitty(image) => image.render_inline(area, buffer),
            Self::Halfblock(image) => image.render(area, buffer),
        }
    }

    /// 只在输出提交阶段消费 Kitty 上传和 placement 状态。
    pub(crate) fn placement_command(&mut self, area: Rect) -> Option<String> {
        match self {
            Self::Kitty(image) => image.placement_command(area),
            _ => None,
        }
    }

    /// 返回该成品持有的像素或协议 payload 字节数；解码原图由独立缓存记账。
    pub(crate) fn resident_bytes(&self) -> u64 {
        match self {
            Self::Kitty(image) => image.resident_bytes(),
            Self::Halfblock(image) => image.resident_bytes(),
        }
    }

    /// 构造缓存测试使用的最小半块成品。
    #[cfg(test)]
    pub(crate) fn test_halfblock() -> Self {
        Self::Halfblock(HalfblockImage::test_image())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::CoverCellFit;
    use image::{DynamicImage, Rgb, RgbImage};
    use ratatui::{buffer::Buffer, layout::Rect};

    use super::PreparedImage;
    use crate::image::{
        graphics::TerminalGraphics,
        key::{ImageIdentity, PixelSize, TerminalImageKey},
    };
    use mineral_model::MediaUrl;

    /// Kitty 截边／拉伸上传目标像素，原样模式上传原图；重复 placement 不重传。
    #[test]
    fn kitty_cell_fit_transmits_the_requested_pixel_dimensions() -> color_eyre::Result<()> {
        let graphics = TerminalGraphics::fixed_kitty();
        let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(301, 199, Rgb([180, 80, 40])));
        let area = Rect::new(0, 0, 13, 4);
        let pixels = PixelSize::from_cells((area.width, area.height), (8, 16));
        for (cell_fit, expected) in [
            (CoverCellFit::Crop, (pixels.width(), pixels.height())),
            (CoverCellFit::Stretch, (pixels.width(), pixels.height())),
            (CoverCellFit::Contain, (source.width(), source.height())),
        ] {
            let identity = ImageIdentity::Url(MediaUrl::remote("https://example.com/cover.png")?);
            let key = if cell_fit == CoverCellFit::Contain {
                TerminalImageKey::source(identity)
            } else {
                TerminalImageKey::rasterized(identity, pixels, (area.width, area.height), cell_fit)
            };
            let mut image = PreparedImage::encode(&source, &key, &graphics)?;
            let mut buffer = Buffer::empty(area);
            image.render(area, &mut buffer);
            image.render(area, &mut buffer);
            let command = image
                .placement_command(area)
                .ok_or_else(|| color_eyre::eyre::eyre!("missing Kitty transmission"))?;
            assert!(command.contains(&format!(",s={},v={};", expected.0, expected.1)));
            image.render(area, &mut buffer);
            assert!(image.placement_command(area).is_none());
        }
        Ok(())
    }
}
