//! 顶部状态行(1 行,无边框):左侧 tabs + 右侧 playback state。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use mineral_task::ChannelFetchKindTag;

use crate::state::{AppState, View};
use crate::theme::Theme;

/// 渲染状态行到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(60)]).areas(area);
    paint_left(frame, left, state, theme);
    paint_right(frame, right, state, theme);
}

/// 左侧:`mineral vX` + `[playlists]` / `[tracks]` tabs,以及 queue 打开时的 `[3 queue]`。
fn paint_left(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let active_pl = state.view == View::Playlists;
    let active_lib = state.view == View::Library;
    let mut spans = vec![
        Span::styled(
            format!("▌ mineral v{}  ", env!("CARGO_PKG_VERSION")),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("│  ", Style::new().fg(theme.surface1)),
        Span::styled("[playlists]", tab_style(active_pl, theme)),
        Span::raw("  "),
        Span::styled("[tracks]", tab_style(active_lib, theme)),
    ];
    if state.queue_open {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("[3 queue]", tab_style(true, theme)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 选中态 tab → text 加粗,未选中态 → overlay 灰。
fn tab_style(active: bool, theme: &Theme) -> Style {
    if active {
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.overlay)
    }
}

/// 右侧:server tasks 按 [`ChannelFetchKindTag`] 拆分 + cover 计数 + 播放状态。
///
/// 显示样:`pl:1 tr:2 song:1 lyr:1 ♥:1 cover:7  ● playing`。各段 N>0 才显示。
/// 全 0 时只剩 glyph,不会假装"什么都没在跑"——封面在跑就显示 cover:N。
fn paint_right(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let pb = &state.playback;
    let (glyph, color, label) = if pb.playing {
        ("●", theme.green, "playing")
    } else {
        ("‖", theme.yellow, "paused")
    };
    let mut spans = Vec::<Span<'_>>::new();
    let by = &state.tasks_snapshot.by_kind;
    // 固定顺序渲染,避免 hashmap 迭代顺序抖动。
    for (tag, label) in [
        (ChannelFetchKindTag::MyPlaylists, "pl"),
        (ChannelFetchKindTag::PlaylistTracks, "tr"),
        (ChannelFetchKindTag::SongUrl, "song"),
        (ChannelFetchKindTag::Lyrics, "lyr"),
        (ChannelFetchKindTag::LikedSongIds, "♥"),
    ] {
        let n = by.get(&tag).copied().unwrap_or(0);
        if n > 0 {
            spans.push(Span::styled(
                format!("{label}:{n} "),
                Style::new().fg(theme.peach),
            ));
        }
    }
    if state.cover_loading > 0 {
        spans.push(Span::styled(
            format!("cover:{} ", state.cover_loading),
            Style::new().fg(theme.peach),
        ));
    }
    spans.push(Span::styled(format!("{glyph} "), Style::new().fg(color)));
    spans.push(Span::styled(label, Style::new().fg(theme.subtext)));
    spans.push(Span::raw(" "));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Right),
        area,
    );
}
