//! 搜索详情面板的外框、标题和列表位置标，以及稳态与下钻过渡的呈现选择。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use super::frame::draw_frame_real;
use super::title;
use super::transition::{SweepArgs, draw_sweep};
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 画 detail 面板：bordered 外框 + 当前栈顶帧。空结果/无栈画空框；滑动期走 sweep 合成。
///
/// # Params:
///   - `border_focused`: 边框是否高亮（焦点环滑动期由调用方置 `false`）
///   - `cover_in_flight`: page morph 封面飞行层已接管头图时置真——跳过自画头图防双画
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    border_focused: bool,
    cover_in_flight: bool,
) {
    let color = if border_focused {
        theme.accent
    } else {
        theme.overlay
    };
    let results = state.channel_search.active_results();
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color))
        .border_type(BorderType::Rounded)
        .title(title::for_panel(state, area.width));
    // 左下角位置标:当前栈顶帧当前区列表 ` n / total `(数据未到 len 0 不显)。detail 非分页
    // (一次拉全),故无 results 列那种 `+`。
    if let Some(dframe) = results.and_then(|kr| kr.detail.current()) {
        let len = dframe.list_len();
        if len > 0 {
            block = block.title_bottom(
                Line::from(detail_position_label(dframe.list().sel(), len))
                    .style(Style::new().fg(theme.overlay)),
            );
        }
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(kr) = results else {
        return;
    };
    if inner.height < 2 || inner.width == 0 {
        return;
    }
    match kr.detail.sweep_frames() {
        Some((from, to, eased, is_push)) => draw_sweep(
            frame,
            inner,
            SweepArgs {
                from,
                to,
                eased,
                is_push,
                cover_in_flight,
            },
            state,
            theme,
        ),
        None => {
            let Some(dframe) = kr.detail.current() else {
                return;
            };
            // 下钻帧(depth>0)头部显示「‹ Esc back」返回提示(spec 二级头部)。
            draw_frame_real(
                frame,
                inner,
                dframe,
                state,
                theme,
                kr.detail.depth() > 0,
                cover_in_flight,
            );
        }
    }
}

/// detail 面板左下角 ` n / total `(1-based 当前位 / 当前区列表长度)。调用方已保证 `total != 0`。
///
/// # Params:
///   - `sel`: 0-based 选中行
///   - `total`: 当前区列表长度
fn detail_position_label(sel: usize, total: usize) -> String {
    format!(" {} / {total} ", sel.saturating_add(1).min(total))
}
