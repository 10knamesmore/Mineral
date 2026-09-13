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

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;

    use super::{AppState, OverlayReveal, Theme, lerp_color};
    use crate::components::popup::component::{Overlay, dock_full_rect, render_overlay};
    use crate::components::popup::queue::QueueOverlay;
    use crate::components::popup::stack::OverlayKind;

    /// 两个同身份副本与一个喜欢条目；过滤会把队列四项压成三项。
    fn queue_state() -> color_eyre::Result<AppState> {
        let mut ctx = AppState::test_default()?;
        ctx.frame_area.set(Rect::new(0, 0, 100, 24));
        ctx.player.queue = ["keep-a", "skip", "keep-a", "keep-z"]
            .into_iter()
            .map(mineral_test::song)
            .collect();
        ctx.player.cursor = mineral_protocol::PlayCursor::InQueue(2);
        ctx.playback.track = ctx.player.queue.get(2).cloned();
        if let Some(song) = ctx.player.queue.last().cloned() {
            ctx.toggle_loved_local(&song);
        }
        Ok(ctx)
    }

    /// 通过真实栈使用的 enum 转发绘制，避免仅测试 QueueOverlay 而漏接边框 hook。
    fn draw(
        kind: &OverlayKind,
        ctx: &AppState,
        theme: &Theme,
        scale: u16,
    ) -> color_eyre::Result<Buffer> {
        let mut terminal = Terminal::new(TestBackend::new(100, 24))?;
        terminal.draw(|frame| {
            render_overlay(frame, frame.area(), kind, scale, true, ctx, theme);
        })?;
        Ok(terminal.backend().buffer().clone())
    }

    /// 过滤后的概览覆盖屏外曲目，只标记实际在播副本；零命中只显示盲文轨道。
    #[test]
    fn filtered_minimap_keeps_exact_playing_occurrence() -> color_eyre::Result<()> {
        let ctx = queue_state()?;
        let theme = crate::test_support::default_theme()?;
        let mut overlay = QueueOverlay::new(0);
        overlay.search.set_query("keep");
        let full = dock_full_rect(ctx.frame_area.get(), &ctx);
        let inner = overlay.block(&ctx, &theme, true).inner(full);
        let x = full.right() - 1;
        let top = inner.y;
        let last = inner.bottom() - 1;
        let mut kind = OverlayKind::Queue(overlay);
        let buffer = draw(&kind, &ctx, &theme, 1000)?;
        // 表头行也是轨道首行：光标停在第一项。
        assert_eq!(buffer.cell((x, top)).map(Cell::symbol), Some("⢹"));
        // 过滤后第二项是在播副本，标记落在轨道中部。
        let middle = (top..=last)
            .find(|&y| buffer.cell((x, y)).is_some_and(|cell| cell.symbol() == "◆"))
            .ok_or_else(|| color_eyre::eyre::eyre!("轨道上应有在播标记"))?;
        assert!(
            middle.abs_diff((top + last) / 2) <= 1,
            "在播标记落在轨道中部"
        );
        // 末项已喜欢：整格染红，右列不留点。
        assert_eq!(buffer.cell((x, last)).map(Cell::symbol), Some("⢸"));
        assert_eq!(buffer.cell((x, last)).map(|cell| cell.fg), Some(theme.red));
        assert_eq!(
            buffer.cell((x, full.bottom() - 1)).map(Cell::symbol),
            Some("╯")
        );
        if let OverlayKind::Queue(overlay) = &mut kind {
            overlay.list.place(1, 0);
        }
        let overlap = draw(&kind, &ctx, &theme, 1000)?;
        // 光标停进在播行：被吸住，那一格换成光标色，整条轨道光晕熄灭。
        assert_eq!(overlap.cell((x, middle)).map(Cell::symbol), Some("◆"));
        assert_eq!(
            overlap.cell((x, middle)).map(|cell| cell.fg),
            Some(theme.accent)
        );
        assert_eq!(
            overlap.cell((x, top)).map(|cell| cell.fg),
            Some(theme.surface1)
        );
        if let OverlayKind::Queue(overlay) = &mut kind {
            overlay.search.set_query("absent");
        }
        let empty = draw(&kind, &ctx, &theme, 1000)?;
        assert!((top..=last).all(|y| empty.cell((x, y)).is_some_and(|cell| cell.symbol() == "⢸")));
        Ok(())
    }

    /// 概览随左侧抽屉平移，并随菜单揭开降低光标强调；右侧抽屉按原几何裁剪。
    #[test]
    fn minimap_follows_dock_compositing_and_focus_yield() -> color_eyre::Result<()> {
        let mut ctx = queue_state()?;
        let theme = crate::test_support::default_theme()?;
        let kind = OverlayKind::queue(0);
        let full = dock_full_rect(ctx.frame_area.get(), &ctx);
        // 轨道首行（表头行）：抽屉平移与焦点让渡都看这一行。
        let y = full.y + 1;
        let sliding = draw(&kind, &ctx, &theme, 500)?;
        let edge = full.x + full.width / 2 - 1;
        assert_eq!(sliding.cell((edge, y)).map(Cell::symbol), Some("⢹"));
        assert_eq!(
            sliding.cell((full.right() - 1, y)).map(Cell::symbol),
            Some(" ")
        );
        ctx.overlay_reveal.set(OverlayReveal {
            own: 1000,
            above: 500,
        });
        let yielded = draw(&kind, &ctx, &theme, 1000)?;
        let expected = lerp_color(theme.accent, theme.subtext, 500, 1000);
        assert_eq!(
            yielded.cell((full.right() - 1, y)).map(|cell| cell.fg),
            Some(expected)
        );
        ctx.overlay_reveal.set(OverlayReveal::default());
        ctx.browse.fullscreen.set(true);
        let right = dock_full_rect(ctx.frame_area.get(), &ctx);
        let partial = draw(&kind, &ctx, &theme, 500)?;
        assert!(
            partial
                .cell((right.right() - 1, y))
                .is_some_and(|cell| cell.symbol() != "⢹")
        );
        let expanded = draw(&kind, &ctx, &theme, 1000)?;
        assert_eq!(
            expanded.cell((right.right() - 1, y)).map(Cell::symbol),
            Some("⢹")
        );
        Ok(())
    }
}
