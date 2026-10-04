//! 常规与全屏歌词按各自排版淡出、淡入。

use ratatui::Frame;
use ratatui::layout::Rect;

use super::LyricsView;
use super::panel::{LyricMode, draw};
use crate::components::layout::shared::transform::{lerp_rect, zero_center};
use crate::components::layout::transition;
use crate::render::anim::ease_in_out;
use crate::render::theme::Theme;

/// 按千分比进度交接完整歌词面板；普通布局没有歌词时仅绘制全屏端。
pub(crate) fn draw_transition(
    frame: &mut Frame<'_>,
    from: Option<Rect>,
    to: Rect,
    state: &LyricsView<'_>,
    theme: &Theme,
    raw_progress: u16,
) {
    if raw_progress == 0 {
        if let Some(from) = from {
            draw(frame, from, state, theme, LyricMode::Compact);
        }
        return;
    }
    if raw_progress >= 1000 {
        draw(frame, to, state, theme, LyricMode::Immersive);
        return;
    }

    let from_buffer = from.map(|area| {
        transition::capture(frame, area, |frame| {
            draw(frame, area, state, theme, LyricMode::Compact);
        })
    });
    let to_buffer = transition::capture(frame, to, |frame| {
        draw(frame, to, state, theme, LyricMode::Immersive);
    });
    let area = lerp_rect(
        from.unwrap_or_else(|| zero_center(to)),
        to,
        ease_in_out(raw_progress),
    );
    transition::panel(
        frame,
        from_buffer.as_ref(),
        Some(&to_buffer),
        area,
        raw_progress,
        theme,
    );
}
