//! 频谱柱画法:每列一根柱 + 渐变色 + 余韵尾迹 + peak cap。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::super::state::{SPECTRUM_RES, SpectrumState};
use crate::render::cells::lower_eighth;
use crate::render::color::lerp_color;
use crate::render::theme::Theme;

/// 渲染整个频谱条阵。
pub(super) fn paint(frame: &mut Frame<'_>, area: Rect, state: &SpectrumState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    // 每根条恒 1 列。FFT 端按 area.width 对数等分桶映射,窗口越宽频率分辨率越细。
    let bar_step: u16 = 1;
    let bar_count = usize::from(area.width).max(1);
    state.target_bars.set(bar_count);
    let total_w = u16::try_from(bar_count)
        .unwrap_or(0)
        .saturating_mul(bar_step);
    // 总宽除不尽时把剩余空格平分到两边,让 spectrum 视觉上居中,避免「左密右稀 / 右密左稀」。
    let pad_left = area.width.saturating_sub(total_w) / 2;
    // state 的条数可能跟新 bar_count 不一致(刚 resize 终端,新 FFT 还没出第一窗),
    // 这一帧只渲染已有的部分,其余留空。下一帧 tick 后对齐。
    let render_count = bar_count.min(state.bar_len()).max(1);
    let max_units = u32::from(area.height) * 8;
    // 渐变跨度。0 → accent(底)、span → accent_2(顶)。area.height-1 给最顶格 100% accent_2。
    let grad_span = u64::from(area.height.saturating_sub(1)).max(1);
    let buf = frame.buffer_mut();
    for col in 0..render_count {
        // 该列底/顶端点色:`Hue` 态全列同色,封面态沿频率轴铺色。
        let endpoints = state.column_colors(col, render_count, theme);
        let palette_lo = endpoints.bottom;
        let palette_hi = endpoints.top;
        let bar = state.bar_at(col);
        let peak = state.spring_peak_at(col);
        let scaled = (u32::from(bar) * max_units) / u32::from(SPECTRUM_RES);
        let full = u16::try_from(scaled / 8).unwrap_or(0);
        let partial = u16::try_from(scaled % 8).unwrap_or(0);
        let peak_scaled = (u32::from(peak) * max_units) / u32::from(SPECTRUM_RES);
        let peak_row = u16::try_from(peak_scaled / 8).unwrap_or(0);
        // bar 顶部所占格(partial > 0 时是 full 行;否则 bar 仅到 full-1 的实心格)。
        let bar_top_row = if partial > 0 {
            full
        } else {
            full.saturating_sub(1)
        };
        // trail 区间 = (bar_top_row, peak_row),即 peak 落下时留在空中的「记忆」。
        // trail_span 包含 peak 自身那格,作为 fade 分母:让最顶 trail 行刚好落在
        // 接近(但不到)peak cap 的色阶,色阶逐行递进,无密度跳变。
        let trail_span = u64::from(peak_row.saturating_sub(bar_top_row)).max(1);
        let x = area.x + pad_left + u16::try_from(col).unwrap_or(0) * bar_step;
        for row_from_bottom in 0..area.height {
            let row_color = lerp_color(
                palette_lo,
                palette_hi,
                u64::from(row_from_bottom),
                grad_span,
            );
            let (glyph, color) = if row_from_bottom < full {
                ("█", row_color)
            } else if row_from_bottom == full && partial > 0 {
                (lower_eighth(u32::from(partial)), row_color)
            } else if *state.cfg().bars().show_trail()
                && row_from_bottom > bar_top_row
                && row_from_bottom < peak_row
            {
                // 余韵:每行往背景色 lerp 一档,d=1 略淡、靠近 peak 几乎融入背景。
                // 单一 glyph(▓)+ 颜色 fade,避免「▓→▒→░」三段密度跳变看起来分层。
                let d = u64::from(row_from_bottom.saturating_sub(bar_top_row));
                let faded = lerp_color(row_color, theme.surface0, d, trail_span);
                ("▓", faded)
            } else {
                continue;
            };
            let y = area.y + area.height.saturating_sub(1 + row_from_bottom);
            for dx in 0..bar_step {
                buf.set_string(x + dx, y, glyph, Style::new().fg(color));
            }
        }

        // peak cap:▔ + theme.text + Bold,跟 bar / trail 的 mauve↔sapphire 拉开。
        // 仅当 peak 严格高于 bar 顶部所占的格才画,避免覆盖 partial glyph 丢失高度信息。
        if *state.cfg().bars().show_peak_cap() && peak_row > bar_top_row && peak_row < area.height {
            let py = area.y + area.height.saturating_sub(1 + peak_row);
            for dx in 0..bar_step {
                buf.set_string(
                    x + dx,
                    py,
                    "▔",
                    Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
                );
            }
        }
    }
}
