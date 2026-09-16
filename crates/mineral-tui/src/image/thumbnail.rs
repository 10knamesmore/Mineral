//! 名称前的 Kitty 行内封面：复用半径预取的低清像素，不从可见行发起下载或完整解码。

use std::io::Write as _;
use std::sync::Arc;

use mineral_model::MediaUrl;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::encode::EncodeRequest;
use super::graphics::GraphicsProtocol;
use super::{ImageEngine, ImageRenderPhase};

impl ImageEngine {
    /// 仅当前生效的 Kitty 协议支持名称前的行内封面。
    pub(crate) fn supports_thumbnails(&self) -> bool {
        self.graphics_protocol() == GraphicsProtocol::Kitty
    }

    /// 在表格高亮完成后绘制一行封面，未缓存时保留背景。
    ///
    /// 只登记低清采样尺寸，不登记 URL 下载或完整解码需求。滚动、离屏合成与区域缩放
    /// 均复用已编码成品，只在稳定阶段提交新编码；逐格占位字符随文本一起平移和裁切。
    ///
    /// # Params:
    ///   - `url`: 该行封面；缺失时仍由表格保留图片列
    ///   - `area`: 表格分配的图片列区域,高度为一行
    ///   - `buf`: 已画完文本与选中高亮的屏幕缓冲
    ///   - `phase`: 当前列表的渲染阶段
    pub(crate) fn render_thumbnail(
        &self,
        url: Option<&MediaUrl>,
        area: Rect,
        buf: &mut Buffer,
        phase: ImageRenderPhase,
    ) {
        if !self.supports_thumbnails() || area.width == 0 || area.height != 1 {
            return;
        }
        self.observe_thumbnail_target();
        let Some(url) = url else {
            return;
        };
        let key = self.thumbnail_preview_key(url);
        if self.terminal_images.render_if_ready(&key, |image| {
            if let Some(command) = image.render_inline(area, buf) {
                self.graphics_commands.borrow_mut().push_str(&command);
            }
        }) || phase != ImageRenderPhase::Stable
            || self.encode_pending.borrow().contains(&key)
        {
            return;
        }

        let mut source = None;
        self.preview_images.render_if_ready(&key, |preview| {
            source = preview.halfblock_source().map(Arc::new);
        });
        let Some(image) = source.or_else(|| self.cache.get(url).cloned()) else {
            return;
        };
        self.encode_pending.borrow_mut().insert(key.clone());
        mineral_log::debug!(target: "cover", url = %url,
            source_width = image.width(), source_height = image.height(),
            cell_pixels = ?self.cell_pixels(), "prepare inline cover thumbnail");
        self.request_encode(EncodeRequest {
            key,
            generation: self.graphics_generation(),
            image,
            target: area,
        });
    }

    /// 在 ratatui 输出本帧 cell 前发送图片指令，每个占位字符保持单格 Unicode 宽度。
    ///
    /// # Error:
    ///   终端输出失败时返回 I/O 错误，与本帧绘制一起终止。
    pub(crate) fn flush_graphics_commands(&self) -> std::io::Result<()> {
        let mut commands = self.graphics_commands.borrow_mut();
        if commands.is_empty() {
            return Ok(());
        }
        let mut output = std::io::stdout().lock();
        output.write_all(commands.as_bytes())?;
        output.flush()?;
        commands.clear();
        Ok(())
    }
}
