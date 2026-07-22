//! Playlists 视图右栏:程序化封面 + 歌单名/meta 两行(居中) + 底部简介行(空显占位)。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::picker::Picker;

use crate::components::layout::shared::cover_image;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use crate::runtime::view_model::PlaylistView;

/// 渲染歌单详情(right pane)到 `area`。
///
/// # Params:
///   - `cover_in_flight`: page morph 封面飞行层已接管主封面时置真——跳过自画封面防双画
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    p: &PlaylistView,
    state: &AppState,
    picker: &Picker,
    theme: &Theme,
    cover_in_flight: bool,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" selected ").style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
    let Some([cover_area, kv_area, footer]) = super::main_cover::sections(area) else {
        return;
    };

    if !cover_in_flight {
        // mineral 聚合歌单无自带封面:拼贴就绪时给合成键,未就绪回落程序化占位。
        let cover = crate::runtime::cover::collage::effective_cover_url(state, &p.data);
        cover_image::render_or_fallback(
            frame,
            cover_area,
            cover.as_ref(),
            state,
            picker,
            theme,
            &p.data.name,
        );
    }

    let total_ms = state.total_duration_ms_of(&p.data.id);
    let len_label = if total_ms == 0 {
        String::from("—")
    } else {
        let total_min = total_ms / 60_000;
        format!("{}h {:02}m", total_min / 60, total_min % 60)
    };

    let src = p.data.source();
    // 标题行:歌单名(text + bold);meta 行:源(源色)· tracks · 总时长(overlay)。居中。
    let kv = vec![
        Line::from(Span::styled(
            p.data.name.clone(),
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                src.label(),
                Style::new().fg(crate::render::theme::resolve_source_color(
                    theme,
                    state.cfg.sources(),
                    src,
                )),
            ),
            Span::styled(
                format!(" · {} tracks · {len_label}", p.data.track_count),
                Style::new().fg(theme.overlay),
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(kv).alignment(Alignment::Center),
        kv_area,
    );

    // 底行:歌单简介首个非空行(overlay,居中截断);无简介显占位——详情面板不放按键
    // 提示(发现交给 ? 帮助浮层),占位语义与 no match found 同款措辞。
    let desc = p
        .data
        .description
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty());
    let footer_line = match desc {
        Some(d) => Line::from(Span::styled(d, Style::new().fg(theme.overlay))),
        None => Line::from(Span::styled(
            "no description",
            Style::new().fg(theme.overlay).add_modifier(Modifier::DIM),
        )),
    };
    frame.render_widget(
        Paragraph::new(footer_line).alignment(Alignment::Center),
        footer,
    );
}
