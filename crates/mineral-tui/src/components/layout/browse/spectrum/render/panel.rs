//! 频谱面板呈现：绘制外框，并按配置风格选择内区画法。

use crate::config::SpectrumStyle;
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
        SpectrumStyle::Bars => bars::paint(frame, inner, state, theme),
    }
}

impl crate::components::lifecycle::Prepare for SpectrumState {
    type Input<'a> = ();

    /// 根据本次布局设置采样分辨率；绘制只消费既有频谱状态。
    fn prepare(&mut self, area: Rect, (): (), _cx: &mut crate::components::frame::PrepareCx<'_>) {
        let inner = Block::new().borders(Borders::ALL).inner(area);
        if !inner.is_empty() {
            self.target_bars = usize::from(inner.width).max(1);
        }
    }
}
