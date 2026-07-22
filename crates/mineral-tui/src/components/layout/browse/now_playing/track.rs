//! Library 视图右栏:程序化封面(以专辑名为种子) + KV + 底部 ▶ 当前曲目。

use mineral_model::SongId;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::picker::Picker;

use crate::components::layout::shared::cover_image;
use crate::components::layout::shared::marquee::MarqueeCtx;
use crate::render::theme::Theme;
use crate::runtime::format::format_ms_opt;
use crate::runtime::marquee::Slot;
use crate::runtime::state::AppState;
use crate::runtime::view_model::SongView;

/// 渲染曲目详情(right pane)到 `area`。
///
/// # Params:
///   - `cover_in_flight`: page morph 封面飞行层已接管主封面时置真——跳过自画封面防双画
#[allow(clippy::too_many_arguments)] // reason: 纯渲染入口,参数即全部输入,收拢成 struct 反而多一层搬运
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    sv: &SongView,
    current_id: Option<&SongId>,
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
    let Some([cover_area, kv_area, current_strip]) = super::main_cover::sections(area) else {
        return;
    };

    if !cover_in_flight {
        let seed = sv
            .data
            .album
            .as_ref()
            .map_or_else(|| sv.data.name.clone(), |a| a.name.clone());
        cover_image::render_or_fallback(
            frame,
            cover_area,
            sv.data.cover_url.as_ref(),
            state,
            picker,
            theme,
            &seed,
        );
    }

    let len = format_ms_opt(sv.data.duration_ms);
    let love_label = if sv.loved { "♥ loved" } else { "♡ —" };
    let love_color = if sv.loved { theme.red } else { theme.overlay };
    let plays_label = match sv.plays {
        Some(n) => n.to_string(),
        None => "—".to_owned(),
    };

    let kv = vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled("length: ", Style::new().fg(theme.overlay)),
            Span::styled(format!("{len:<10}"), Style::new().fg(theme.text)),
            Span::styled("plays:  ", Style::new().fg(theme.overlay)),
            Span::styled(plays_label, Style::new().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled("love: ", Style::new().fg(theme.overlay)),
            Span::styled(love_label, Style::new().fg(love_color)),
        ]),
    ];
    frame.render_widget(Paragraph::new(kv), kv_area);

    let is_current = current_id.is_some_and(|cid| cid == &sv.data.id);
    let style = if is_current {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.overlay)
    };
    let artist = sv
        .data
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    // ▶ 歌名 (别名) — 艺人:别名**跟随整行 style**(playing 与两侧一起 accent 高亮、否则一起
    // dim),靠括号而非颜色表达次级——避免像其它列表那样单独染 dim 造成高亮态 bright-dim-bright 断层。
    let alias = sv
        .data
        .alias
        .as_deref()
        .map(|a| format!(" ({a})"))
        .unwrap_or_default();
    let strip = MarqueeCtx::new(state, theme, /*fade_to*/ theme.base).line(
        vec![Span::styled(
            format!(" ▶ {}{alias} — {artist} ", sv.data.name),
            style,
        )],
        Slot::NowPlaying,
        &sv.data.id.qualified(),
        current_strip.width,
    );
    frame.render_widget(Paragraph::new(strip), current_strip);
}
