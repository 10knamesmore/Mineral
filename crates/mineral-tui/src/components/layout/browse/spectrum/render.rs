//! 频谱渲染入口:画面板 Block 后按配置风格分发内区画法。

mod bars;
mod scope;
mod terrain;
mod waterfall;

use mineral_config::SpectrumStyle;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use super::state::SpectrumState;
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

/// Braille 点阵掩码:`BRAILLE_BITS[py % 4][px % 2]`,基码 U+2800。
const BRAILLE_BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// Braille 点阵画布:每字符格 2×4 点,亚格分辨率画连续曲线用。
///
/// 一格只有一个前景色,**首写优先**——先落笔的层持有该格颜色。配合"前景层
/// 先画"的绘制顺序,重叠格自然显前景色。
struct BrailleGrid {
    /// 画布宽(字符格)。
    cols: usize,

    /// 画布高(字符格)。
    rows: usize,

    /// 每格的点位掩码(0 = 全空,不落笔)。
    dots: Vec<u8>,

    /// 每格的首写颜色。
    colors: Vec<Option<Color>>,
}

impl BrailleGrid {
    /// 建空画布。点分辨率 = `cols × 2` 横向、`rows × 4` 纵向。
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            dots: vec![0; cols * rows],
            colors: vec![None; cols * rows],
        }
    }

    /// 点亮 `(px, py)`(点坐标,越界忽略);该格未着色时记下 `color`。
    fn set(&mut self, px: usize, py: usize, color: Color) {
        let (cx, cy) = (px / 2, py / 4);
        if cx >= self.cols || cy >= self.rows {
            return;
        }
        let idx = cy * self.cols + cx;
        let bit = BRAILLE_BITS
            .get(py % 4)
            .and_then(|row| row.get(px % 2))
            .copied()
            .unwrap_or(0);
        if let Some(dot) = self.dots.get_mut(idx) {
            *dot |= bit;
        }
        if let Some(slot) = self.colors.get_mut(idx) {
            slot.get_or_insert(color);
        }
    }

    /// 把非空格写进 buffer(左上角对齐 `area`)。
    fn blit(&self, buf: &mut Buffer, area: Rect) {
        for cy in 0..self.rows.min(usize::from(area.height)) {
            for cx in 0..self.cols.min(usize::from(area.width)) {
                let idx = cy * self.cols + cx;
                let mask = self.dots.get(idx).copied().unwrap_or(0);
                if mask == 0 {
                    continue;
                }
                let Some(glyph) = char::from_u32(0x2800 + u32::from(mask)) else {
                    continue;
                };
                let color = self
                    .colors
                    .get(idx)
                    .copied()
                    .flatten()
                    .unwrap_or(Color::Reset);
                let x = area.x + u16::try_from(cx).unwrap_or(0);
                let y = area.y + u16::try_from(cy).unwrap_or(0);
                buf.set_string(x, y, glyph.to_string(), Style::new().fg(color));
            }
        }
    }
}
