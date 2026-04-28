//! Spectrum 频谱面板:伪随机状态机 + 半字符精度块渲染。
//!
//! 没有真正的 FFT —— stage 5 只用 LCG 生成"看起来像音频"的波形,
//! 后续接真音频时把 [`SpectrumState::tick`] 的内容换成 FFT 输出即可。

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

/// 频谱柱条的逻辑分辨率(每格 1/8 字符高度,共 8 行 × 8 = 64 单位)。
const SPECTRUM_RES: u16 = 64;
/// 内部 ringbuffer 容量(渲染时按 area.width 截取实际显示数)。
const SPECTRUM_BARS: usize = 64;

/// 频谱状态:每根柱子的当前高度(1/8 字符单位,0..=[`SPECTRUM_RES`])。
#[derive(Clone, Debug)]
pub struct SpectrumState {
    bars: [u16; SPECTRUM_BARS],
    seed: u32,
}

impl SpectrumState {
    /// 初始状态(全部静默,固定种子保证启动一致)。
    pub fn new() -> Self {
        Self {
            bars: [0; SPECTRUM_BARS],
            seed: 0x1357_9bdf,
        }
    }

    /// 一次 tick:`playing=true` 用 LCG 生成新目标 + 平滑插值,
    /// `playing=false` 让所有柱子向 0 衰减。
    pub fn tick(&mut self, playing: bool) {
        if !playing {
            for b in &mut self.bars {
                *b = b.saturating_sub(*b / 4 + 1);
            }
            return;
        }
        for i in 0..self.bars.len() {
            self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let r = u16::try_from((self.seed >> 16) & 0x7fff).unwrap_or(0);
            let target_u32 = (u32::from(r) * u32::from(SPECTRUM_RES)) / 0x8000;
            let target =
                u16::try_from(target_u32.min(u32::from(SPECTRUM_RES))).unwrap_or(SPECTRUM_RES);
            if let Some(b) = self.bars.get_mut(i) {
                let blended = (u32::from(*b) * 7 + u32::from(target) * 3) / 10;
                *b = u16::try_from(blended).unwrap_or(*b);
            }
        }
    }
}

impl Default for SpectrumState {
    fn default() -> Self {
        Self::new()
    }
}

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

    let labels_h = if inner.height >= 3 { 1 } else { 0 };
    let bars_h = inner.height.saturating_sub(labels_h);
    let bars_area = Rect::new(inner.x, inner.y, inner.width, bars_h);
    paint_bars(frame, bars_area, state, theme);

    if labels_h == 1 {
        let label_area = Rect::new(inner.x, inner.y + bars_h, inner.width, 1);
        paint_labels(frame, label_area, theme);
    }
}

fn paint_bars(frame: &mut Frame<'_>, area: Rect, state: &SpectrumState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let bar_step: u16 = if area.width >= 64 { 2 } else { 1 };
    let bar_count = usize::from(area.width / bar_step);
    let max_units = u32::from(area.height) * 8;
    let buf = frame.buffer_mut();
    for col in 0..bar_count {
        let bar = state.bars.get(col).copied().unwrap_or(0);
        let scaled = (u32::from(bar) * max_units) / u32::from(SPECTRUM_RES);
        let full = u16::try_from(scaled / 8).unwrap_or(0);
        let partial = u16::try_from(scaled % 8).unwrap_or(0);
        let x = area.x + u16::try_from(col).unwrap_or(0) * bar_step;
        for row_from_bottom in 0..area.height {
            let glyph = if row_from_bottom < full {
                "█"
            } else if row_from_bottom == full && partial > 0 {
                partial_glyph(partial)
            } else {
                continue;
            };
            let y = area.y + area.height.saturating_sub(1 + row_from_bottom);
            let color = if u32::from(row_from_bottom) * 100 > u32::from(area.height) * 35 {
                theme.accent_2
            } else {
                theme.accent
            };
            buf.set_string(x, y, glyph, Style::new().fg(color));
        }
    }
}

fn partial_glyph(units: u16) -> &'static str {
    match units {
        1 => "▁",
        2 => "▂",
        3 => "▃",
        4 => "▄",
        5 => "▅",
        6 => "▆",
        _ => "▇",
    }
}

fn paint_labels(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    if area.width < 12 {
        return;
    }
    let spaces = " ".repeat(usize::from(area.width).saturating_sub(9));
    let line = Line::from(format!("20Hz{spaces}20kHz")).style(Style::new().fg(theme.overlay));
    frame.render_widget(Paragraph::new(line), area);
}
