//! 名称前的 Kitty 行内封面：复用半径预取的低清像素，不从可见行发起下载或完整解码。

use std::sync::Arc;

use mineral_model::MediaUrl;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::encode::EncodeRequest;
use super::graphics::GraphicsProtocol;
use super::{ImageEngine, ImageRenderPhase};

/// 转场保留的行内图片输入；身份与尺寸固定，资源由准备入口保活。
#[derive(Clone)]
pub(crate) struct InlineImage {
    /// 编码成品的缓存身份。
    key: super::key::TerminalImageKey,

    /// 已就绪的 Kitty 图片身份。
    image_id: u32,

    /// 占位字符的原始区域。
    area: Rect,

    /// 编码所属终端协议代次，切换协议后不能继续使用。
    generation: u64,
}

impl InlineImage {
    /// 把固定身份写入缓冲，不触发加载或协议发送。
    pub(crate) fn paint(&self, target: &mut Buffer) {
        super::kitty::paint_inline(self.area, target, self.image_id);
    }
}

impl ImageEngine {
    /// 测试中模拟一次行内图片的准备和绘制。
    #[cfg(test)]
    pub(crate) fn prepare_and_render_thumbnail(
        &mut self,
        url: Option<&MediaUrl>,
        area: Rect,
        buf: &mut Buffer,
        phase: ImageRenderPhase,
    ) {
        self.begin_preparation();
        self.prepare_thumbnail(url, area, phase);
        self.finish_preparation();
        self.render_thumbnail(url, area, buf);
    }

    /// 保留当前已就绪缩略图的呈现输入；未就绪图片不进入旧端快照。
    pub(crate) fn inline_image(&self, url: &MediaUrl, area: Rect) -> Option<InlineImage> {
        let key = self.thumbnail_preview_key(url);
        let mut image_id = None;
        self.terminal_images
            .render_if_ready(&key, |image| image_id = image.inline_id());
        Some(InlineImage {
            key,
            image_id: image_id?,
            area,
            generation: self.graphics_generation(),
        })
    }

    /// 协议变更后，旧端图片描述不再属于当前终端能力。
    pub(crate) fn inline_is_current(&self, image: &InlineImage) -> bool {
        image.generation == self.graphics_generation()
    }

    /// 旧视图仍参与转场时保留其成品，并在实际提交时保证 placement 就绪。
    pub(crate) fn retain_inline(&mut self, image: &InlineImage) {
        self.terminal_images.observe(&image.key);
        self.graphics_placements
            .push((image.key.clone(), image.area));
    }

    /// 仅当前生效的 Kitty 协议支持名称前的行内封面。
    pub(crate) fn supports_thumbnails(&self) -> bool {
        self.graphics_protocol() == GraphicsProtocol::Kitty
    }

    /// 在表格高亮完成后绘制一行封面，未缓存时保留背景。
    ///
    /// 只读取已编码成品；逐格占位字符随文本一起平移和裁切。
    ///
    /// # Params:
    ///   - `url`: 该行封面；缺失时仍由表格保留图片列
    ///   - `area`: 表格分配的图片列区域,高度为一行
    ///   - `buf`: 已画完文本与选中高亮的屏幕缓冲
    pub(crate) fn render_thumbnail(&self, url: Option<&MediaUrl>, area: Rect, buf: &mut Buffer) {
        if !self.supports_thumbnails() || area.width == 0 || area.height != 1 {
            return;
        }
        if let Some(url) = url {
            let key = self.thumbnail_preview_key(url);
            self.terminal_images
                .render_if_ready(&key, |image| image.render_inline(area, buf));
        }
    }

    /// 协调行内封面的成品需求；下载仍由预取策略决定。
    pub(crate) fn prepare_thumbnail(
        &mut self,
        url: Option<&MediaUrl>,
        area: Rect,
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
        self.terminal_images.observe(&key);
        if self.terminal_images.contains(&key) {
            self.graphics_placements.push((key, area));
            return;
        }
        if phase != ImageRenderPhase::Stable || self.encode_pending.contains(&key) {
            return;
        }
        let mut source = None;
        self.preview_images.observe(&key);
        self.preview_images.render_if_ready(&key, |preview| {
            source = preview.halfblock_source().map(Arc::new);
        });
        let Some(image) = source.or_else(|| self.cache.get(url).cloned()) else {
            return;
        };
        self.encode_pending.insert(key.clone());
        mineral_log::debug!(target: "cover", url = %url, "prepare inline cover thumbnail");
        self.request_encode(EncodeRequest {
            key,
            generation: self.graphics_generation(),
            image,
            target: area,
        });
    }

    /// 消费本次实际提交所需的协议指令；重复绘制不会调用此入口。
    pub(crate) fn take_graphics_commands(&mut self) -> String {
        let mut commands = String::new();
        for (key, area) in &self.graphics_placements {
            if let Some(command) = self.terminal_images.placement_command(key, *area) {
                commands.push_str(&command);
            }
        }
        commands
    }
}
