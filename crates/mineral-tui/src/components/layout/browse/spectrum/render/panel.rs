//! 频谱面板呈现：绘制外框，并按配置风格选择内区画法。

use mineral_config::SpectrumStyle;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use super::super::state::SpectrumState;
use super::{bars, scope, terrain, waterfall};
use crate::render::theme::Theme;

/// 渲染频谱到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &SpectrumState, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" spectrum ").style(Style::new().fg(theme.subtext)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    match state.cfg().style() {
        SpectrumStyle::Scope => scope::paint(frame, inner, state, theme),
        SpectrumStyle::Waterfall => waterfall::paint(frame, inner, state, theme),
        SpectrumStyle::Terrain => terrain::paint(frame, inner, state, theme),
        // 枚举在上游 non_exhaustive,wildcard 必需:未知新风格回落默认条形。
        _ => bars::paint(frame, inner, state, theme),
    }
}
