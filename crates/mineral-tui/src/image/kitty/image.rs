//! Kitty 终端图片成品：shared memory 资源、image id 与 Unicode placement。

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::command::{create_virtual_placement, transmit_shared_memory};
use super::pixels::PixelData;
use super::shared_memory::SharedMemory;
use crate::image::graphics::TerminalRelay;

/// 一张经 POSIX shared memory 发送一次、可按不同 placement 尺寸显示的 Kitty 图片。
pub(crate) struct KittyImage {
    /// 非零 Kitty image id。
    image_id: u32,

    /// 首次 placement 前尚未写给终端的 shared memory 传输命令。
    transmission: Option<String>,

    /// 最近排入输出的 placement；cell 尺寸变化时重建,图片像素继续复用。
    placement: Option<u32>,

    /// shared memory 资源句柄，保留到终端读取或缓存成品逐出。
    resource: SharedMemory,

    /// 图形控制序列的终端 relay 形态。
    relay: TerminalRelay,
}

impl KittyImage {
    /// 把解码原图写入 shared memory 并创建 Kitty 成品。
    ///
    /// # Params:
    ///   - `source`: 已解码原图
    ///   - `image_id`: 非零 Kitty image id
    ///   - `relay`: 终端 relay 形态
    ///
    /// # Return:
    ///   只按图片身份缓存、显示尺寸由 placement 决定的 Kitty 成品
    ///
    /// # Error:
    ///   shared memory 创建或写入失败时返回错误
    pub(crate) fn encode(
        source: &DynamicImage,
        image_id: u32,
        relay: TerminalRelay,
    ) -> color_eyre::Result<Self> {
        let pixels = PixelData::from_image(source);
        let resource = SharedMemory::create(image_id, &pixels.bytes)?;
        let transmission = transmit_shared_memory(
            image_id,
            (source.width(), source.height()),
            pixels.format,
            resource.name(),
            relay,
        );
        mineral_log::debug!(target: "cover", image_id,
            width = source.width(), height = source.height(),
            pixel_format = ?pixels.format, payload_bytes = resource.resident_bytes(),
            "Kitty image prepared");
        Ok(Self {
            image_id,
            transmission: Some(transmission),
            placement: None,
            resource,
            relay,
        })
    }

    /// 绘制逐格 Unicode 封面；首次传输图片，尺寸变化时更新 placement。
    pub(crate) fn render_inline(&mut self, area: Rect, buffer: &mut Buffer) -> Option<String> {
        if area.is_empty() {
            return None;
        }
        let cells = super::placement::clamp_cells((area.width, area.height));
        let placement_id = super::placement::render_inline(area, buffer, self.image_id);
        if self.placement == Some(placement_id) {
            return None;
        }
        self.placement = Some(placement_id);
        let mut command = self.transmission.take().unwrap_or_default();
        command.push_str(&create_virtual_placement(
            self.image_id,
            placement_id,
            cells,
            self.relay,
        ));
        Some(command)
    }

    /// 返回 RGB / RGBA shared memory 与尚未发送的控制序列占用，不重复计入解码原图。
    pub(crate) fn resident_bytes(&self) -> u64 {
        self.resource.resident_bytes().saturating_add(
            self.transmission.as_ref().map_or(0, |command| {
                u64::try_from(command.capacity()).unwrap_or(u64::MAX)
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;

    use image::{DynamicImage, GrayAlphaImage, LumaA, Rgb, RgbImage, Rgba, RgbaImage};
    use nix::fcntl::OFlag;
    use nix::sys::mman::shm_open;
    use nix::sys::stat::{Mode, fstat};

    use super::KittyImage;
    use crate::image::graphics::{TerminalGraphics, TerminalRelay};

    /// 终端声明的格式、shared memory 实际长度和缓存字节数必须一致。
    #[test]
    fn transmission_matches_shared_memory_layout() -> color_eyre::Result<()> {
        let graphics = TerminalGraphics::fixed((8, 16));
        // RGB / RGBA payload 均按 64 KiB 对齐，避免 fstat 的页对齐长度混入像素记账断言。
        for (source, format, payload_bytes) in [
            (
                DynamicImage::ImageRgb8(RgbImage::from_pixel(256, 256, Rgb([10, 20, 30]))),
                ",f=24,",
                256 * 256 * 3_u64,
            ),
            (
                DynamicImage::ImageRgba8(RgbaImage::from_pixel(256, 256, Rgba([10, 20, 30, 128]))),
                ",f=32,",
                256 * 256 * 4,
            ),
            (
                DynamicImage::ImageLumaA8(GrayAlphaImage::from_pixel(256, 256, LumaA([42, 64]))),
                ",f=32,",
                256 * 256 * 4,
            ),
        ] {
            let image = KittyImage::encode(
                &source,
                graphics.allocate_kitty_image_id(),
                TerminalRelay::Direct,
            )?;
            let transmission = image
                .transmission
                .as_ref()
                .ok_or_else(|| color_eyre::eyre::eyre!("missing Kitty transmission"))?;
            assert!(transmission.contains(format), "{transmission:?}");
            let fd = shm_open(image.resource.name(), OFlag::O_RDONLY, Mode::empty())?;
            assert_eq!(
                u64::try_from(fstat(fd.as_raw_fd())?.st_size)?,
                payload_bytes
            );
            assert_eq!(image.resource.resident_bytes(), payload_bytes);
            assert_eq!(
                image.resident_bytes(),
                payload_bytes + u64::try_from(transmission.capacity())?,
            );
        }
        Ok(())
    }
}
