//! Kitty 终端图片成品：shared memory 资源、image id 与 Unicode placement。

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::command::transmit_shared_memory;
use super::placement::render;
use super::shared_memory::SharedMemory;
use crate::image::graphics::TerminalRelay;

/// 一张经 POSIX shared memory 发送一次、可按不同 placement 尺寸显示的 Kitty 图片。
pub(crate) struct KittyImage {
    /// 非零 Kitty image id。
    image_id: u32,

    /// 首次 placement 前尚未写给终端的 shared memory 传输命令。
    transmission: Option<String>,

    /// shared memory 资源句柄，保留到终端读取或缓存成品逐出。
    _resource: SharedMemory,

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
        let rgba = source.to_rgba8();
        let resource = SharedMemory::create(image_id, rgba.as_raw())?;
        let transmission = transmit_shared_memory(
            image_id,
            (rgba.width(), rgba.height()),
            resource.name(),
            relay,
        );
        Ok(Self {
            image_id,
            transmission: Some(transmission),
            _resource: resource,
            relay,
        })
    }

    /// 按当前区域创建或复用 virtual placement，并写入 Unicode placeholders。
    pub(crate) fn render(&mut self, area: Rect, buffer: &mut Buffer) {
        render(
            area,
            buffer,
            self.image_id,
            &mut self.transmission,
            self.relay,
        );
    }
}
