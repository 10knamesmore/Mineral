//! 搜索详情面板的外框、标题和列表位置标，以及稳态与下钻过渡的呈现选择。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use super::frame::draw_frame_real;
use super::placeholder::loading_glyph;
use super::title;
use super::transition::{SweepArgs, draw_sweep};
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, ArtistAlbums};

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
    // 艺人专辑未收齐时以 `+` 标记已加载数量，续页等待期间保留列表并显示 loading。
    if let Some(dframe) = results.and_then(|kr| kr.detail.current()) {
        let len = dframe.list_len();
        let albums = dframe.current_album_list();
        let has_more = albums.is_some_and(ArtistAlbums::has_more);
        if len > 0 || has_more {
            let mut label = detail_position_label(dframe.list().sel(), len, has_more);
            if albums.is_some_and(ArtistAlbums::is_loading) {
                label.push_str(loading_glyph(state));
                label.push_str(" loading ");
            }
            block = block.title_bottom(Line::from(label).style(Style::new().fg(theme.overlay)));
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

/// detail 面板的位置标；尚有下一页时在已加载数量后加 `+`，空页当前位置为 0。
fn detail_position_label(sel: usize, total: usize, has_more: bool) -> String {
    let more = if has_more { "+" } else { "" };
    format!(" {} / {total}{more} ", sel.saturating_add(1).min(total))
}
