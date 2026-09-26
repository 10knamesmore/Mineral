//! 队列右边框的全列表概览；标记按过滤视图排列，在播身份由真实队列锚点决定。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::QueueOverlay;
use crate::components::layout::shared::list_minimap::{
    MinimapCursor, MinimapEntry, render_minimap,
};
use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::{AppState, OverlayReveal};

impl QueueOverlay {
    /// 在完整面板的右边框绘制过滤列表的位置与标记，从表头行到底行、不含圆角。
    ///
    /// # Params:
    ///   - `buf`: 包含面板边框的目标，随后可由抽屉动画裁剪和平移。
    ///   - `area`: 完整面板矩形。
    ///   - `inner`: 同一外框计算出的内容区。
    ///   - `ctx`: 当前队列、喜欢状态、配置与焦点让渡进度。
    ///   - `theme`: 本帧主题，光标强调会随上层菜单展开而变暗。
    pub(super) fn render_minimap(
        &self,
        buf: &mut Buffer,
        area: Rect,
        inner: Rect,
        ctx: &AppState,
        theme: &Theme,
    ) {
        let visible = self.visible(ctx);
        let current = ctx.queue_current_index();
        let cursor = MinimapCursor::new(
            &self.list,
            visible.len(),
            ScrollMotion::Advancing {
                scrolloff: ctx.scrolloff(),
                glide_ticks: ctx.list_glide_ticks(),
            },
            ctx.minimap_cursor_ticks(),
            ctx.cfg.tui().minimap(),
        );
        let entries = visible.iter().enumerate().filter_map(|(index, &raw)| {
            ctx.player.queue.get(raw).map(|song| MinimapEntry {
                index,
                loved: ctx.is_liked(song),
                playing: current == Some(raw),
            })
        });
        // 光标强调与选中行同步让渡给上层菜单，喜欢和在播标记仍可读。
        let minimap_theme = Theme {
            accent: lerp_color(
                theme.accent,
                theme.subtext,
                u64::from(ctx.overlay_reveal.get().yielded()),
                u64::from(OverlayReveal::FULL),
            ),
            ..*theme
        };
        render_minimap(
            buf,
            Rect::new(
                area.right().saturating_sub(1),
                inner.y,
                area.width.min(1),
                inner.height,
            ),
            visible.len(),
            cursor,
            entries,
            &minimap_theme,
        );
    }
}
