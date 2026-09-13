//! 常规与全屏歌词按端点排版淡化，双方可见的当前原文行独立移动。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::widgets::Paragraph;

use super::panel::{CurrentLine, LyricMode, SCROLL_FULL, WindowLayout, paint_panel};
use crate::components::layout::shared::text::center_bg;
use crate::components::layout::shared::transform::{lerp_rect, zero_center};
use crate::components::layout::transition;
use crate::render::anim::{ease_in_out, lerp_u16};
use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 同一帧的歌词转场：先画面板，其他持续内容绘制后再叠共享当前行。
/// 两端排版在构造时确定，两次绘制共用可见行和高亮上下文。
pub(crate) struct LyricTransition<'a> {
    /// 常规端点；紧凑终端没有歌词面板时缺席。
    from: Option<Endpoint<'a>>,

    /// 全屏端点，使用 Immersive 排版。
    to: Endpoint<'a>,

    /// 当前帧的歌词、播放与面板标题状态。
    state: &'a AppState,

    /// 未缓动千分比，0 为常规端、1000 为全屏端。
    raw_progress: u16,
}

/// 一个端点的稳定面板区域与已完成定位的歌词窗口。
struct Endpoint<'a> {
    /// 包含边框的面板区域。
    area: Rect,

    /// 无歌词或内区为空时没有窗口，绘制仍保留面板的空态。
    window: Option<WindowLayout<'a>>,
}

impl<'a> Endpoint<'a> {
    /// 按端点尺寸与呈现模式计算一次窗口排版。
    fn new(area: Rect, state: &'a AppState, mode: LyricMode) -> Self {
        Self {
            area,
            window: WindowLayout::for_panel(area, state, mode),
        }
    }

    /// 绘制端点内容；共享原文行仅省略绘制，仍保留占位。
    fn draw(
        &self,
        frame: &mut Frame<'_>,
        state: &AppState,
        theme: &Theme,
        omit_primary: Option<usize>,
    ) {
        paint_panel(
            frame,
            self.area,
            state,
            theme,
            self.window.as_ref(),
            omit_primary,
        );
    }
}

impl<'a> LyricTransition<'a> {
    /// 准备同一帧的两端布局，供面板和共享当前行分层绘制。
    ///
    /// # Params:
    ///   - `from`: 常规歌词端点；紧凑终端没有歌词面板时为 `None`。
    ///   - `to`: 全屏歌词端点。
    ///   - `state`: 本帧歌词与播放状态。
    ///   - `raw_progress`: 未缓动的千分比，0 为常规端、1000 为全屏端。
    pub(crate) fn new(
        from: Option<Rect>,
        to: Rect,
        state: &'a AppState,
        raw_progress: u16,
    ) -> Self {
        Self {
            from: from.map(|area| Endpoint::new(area, state, LyricMode::Compact)),
            to: Endpoint::new(to, state, LyricMode::Immersive),
            state,
            raw_progress,
        }
    }

    /// 绘制歌词面板；中帧省略共享当前行，稳态端点完整绘制一次。
    pub(crate) fn draw_panel(&self, frame: &mut Frame<'_>, theme: &Theme) {
        if self.raw_progress == 0 {
            if let Some(from) = &self.from {
                from.draw(frame, self.state, theme, None);
            }
            return;
        }
        if self.raw_progress >= 1000 {
            self.to.draw(frame, self.state, theme, None);
            return;
        }

        let omit_primary = self.shared_current().map(|(from, _)| from.line_index());
        let from_buffer = self.from.as_ref().map(|from| {
            transition::capture(frame, from.area, |frame| {
                from.draw(frame, self.state, theme, omit_primary);
            })
        });
        let to_buffer = transition::capture(frame, self.to.area, |frame| {
            self.to.draw(frame, self.state, theme, omit_primary);
        });
        let from = self
            .from
            .as_ref()
            .map_or_else(|| zero_center(self.to.area), |from| from.area);
        let area = lerp_rect(from, self.to.area, ease_in_out(self.raw_progress));
        transition::panel(
            frame,
            from_buffer.as_ref(),
            Some(&to_buffer),
            area,
            self.raw_progress,
            theme,
        );
    }

    /// 在其他持续内容之后叠共享当前行；稳态或缺少真实双端当前行时不绘制。
    pub(crate) fn draw_current(&self, frame: &mut Frame<'_>, theme: &Theme) {
        if let Some((from, to)) = self.shared_current() {
            paint_shared_line(frame, from, to, ease_in_out(self.raw_progress), theme);
        }
    }

    /// 两次绘制共用此判定，只有中帧且双方可见同一原文行才交给共享层。
    fn shared_current(&self) -> Option<(CurrentLine<'_>, CurrentLine<'_>)> {
        if self.raw_progress == 0 || self.raw_progress >= 1000 {
            return None;
        }
        let from = self.from.as_ref()?.window.as_ref()?.current_line()?;
        let to = self.to.window.as_ref()?.current_line()?;
        (from.line_index() == to.line_index()).then_some((from, to))
    }
}

/// 按真实行位置移动同一原文行，行级高亮在两端样式间过渡，逐字 wipe 仍由播放驱动。
fn paint_shared_line(
    frame: &mut Frame<'_>,
    from: CurrentLine<'_>,
    to: CurrentLine<'_>,
    progress: u16,
    theme: &Theme,
) {
    let area = lerp_rect(from.area(), to.area(), progress);
    let background = center_bg(frame, area);
    let mut line = from.render(theme, background);
    let target = to.render(theme, background);
    // 整行歌词在 Line 上着色；逐字歌词的颜色和加粗都在 Span 上，两端相同，直接保留。
    if let (Some(from_color), Some(to_color)) = (line.style.fg, target.style.fg) {
        line.style = line
            .style
            .fg(lerp_color(from_color, to_color, u64::from(progress), 1000))
            .remove_modifier(Modifier::BOLD);
        let emphasis = lerp_u16(from.emphasis(), to.emphasis(), progress);
        if emphasis > SCROLL_FULL / 2 {
            line.style = line.style.add_modifier(Modifier::BOLD);
        }
    }
    // 只清理当前原文实际占用的宽度，保留两侧经过的播放信息。
    let width = u16::try_from(line.width())
        .unwrap_or(area.width)
        .min(area.width);
    let text_area = Rect::new(area.x + (area.width - width) / 2, area.y, width, 1);
    transition::clear_symbols(frame.buffer_mut(), text_area);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), text_area);
}
