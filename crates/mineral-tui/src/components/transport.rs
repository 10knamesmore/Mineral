//! Transport 面板:now-line / 进度条 / 控制按钮 / vol·mode。

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::playback::{format_ms, Playback};
use crate::theme::Theme;

/// 渲染 Transport 面板到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" transport ").style(Style::new().fg(theme.subtext)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let [now, meta, prog, ctrl, vms, _filler] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    paint_now(frame, now, pb, theme);
    paint_meta(frame, meta, pb, theme);
    paint_progress(frame, prog, pb, theme);
    paint_controls(frame, ctrl, pb, theme);
    paint_vol_mode(frame, vms, pb, theme);
}

fn paint_now(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let title = pb.track.as_ref().map_or("—", |t| t.name.as_str());
    let line = Line::from(title.to_owned())
        .style(Style::new().fg(theme.text).add_modifier(Modifier::BOLD));
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn paint_meta(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let artist = pb
        .track
        .as_ref()
        .and_then(|t| t.artists.first())
        .map_or("", |a| a.name.as_str());
    let album = pb
        .track
        .as_ref()
        .and_then(|t| t.album.as_ref())
        .map_or("", |a| a.name.as_str());
    let line = Line::from(format!("{artist} · {album}")).style(
        Style::new()
            .fg(theme.subtext)
            .add_modifier(Modifier::ITALIC),
    );
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn paint_progress(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    if area.height == 0 || area.width < 12 {
        return;
    }
    let elapsed = format_ms(pb.position_ms);
    let total = format_ms(pb.duration_ms());
    // 留 elapsed + 1 + (bar) + 1 + total + 1*2 padding
    let reserve = u16::try_from(elapsed.len() + total.len() + 4).unwrap_or(area.width);
    let bar_w = usize::from(area.width.saturating_sub(reserve));
    if bar_w == 0 {
        return;
    }
    let filled = bar_w * usize::from(pb.ratio_bps()) / 10_000;
    let fill = "━".repeat(filled);
    let mut spans = vec![
        Span::styled(format!(" {elapsed} "), Style::new().fg(theme.accent)),
        Span::styled(fill, Style::new().fg(theme.accent_2)),
    ];
    if filled < bar_w {
        spans.push(Span::styled(
            "●",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
        let track = "─".repeat(bar_w.saturating_sub(filled).saturating_sub(1));
        spans.push(Span::styled(track, Style::new().fg(theme.surface0)));
    }
    spans.push(Span::styled(
        format!(" {total} "),
        Style::new().fg(theme.subtext),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn paint_controls(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    if area.height < 2 {
        return;
    }
    let [btn, lbl] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let play_glyph = if pb.playing { "⏸" } else { "▶" };
    let mode_glyph = pb.mode.glyph();
    let buttons = format!("[⏮]   [{play_glyph}]   [⏭]   [{mode_glyph}]");
    let labels = "p      ␣        n      m";
    frame.render_widget(
        Paragraph::new(Line::from(buttons).style(Style::new().fg(theme.text)))
            .alignment(Alignment::Center),
        btn,
    );
    frame.render_widget(
        Paragraph::new(Line::from(labels).style(Style::new().fg(theme.overlay)))
            .alignment(Alignment::Center),
        lbl,
    );
}

fn paint_vol_mode(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    const BAR_W: usize = 10;
    let filled = usize::from(pb.volume_pct) * BAR_W / 100;
    let fill = "█".repeat(filled);
    let empty = "░".repeat(BAR_W.saturating_sub(filled));
    // PlayUrl 在 PlayUrlReady 或 prefetch 命中后写入,切歌瞬间清成 None。
    // 没拿到时显 `—`,跟 transport 别处的「无值」表示一致。
    let fmt_text = pb
        .play_url
        .as_ref()
        .map(|pu| format!("{} {}kbps", pu.format, pu.bitrate_bps / 1000))
        .unwrap_or_else(|| "—".to_owned());
    let line = Line::from(vec![
        Span::styled(" vol ", Style::new().fg(theme.overlay)),
        Span::styled(fill, Style::new().fg(theme.accent)),
        Span::styled(empty, Style::new().fg(theme.surface0)),
        Span::styled(
            format!(" {}%", pb.volume_pct),
            Style::new().fg(theme.subtext),
        ),
        Span::styled("   │   ", Style::new().fg(theme.surface1)),
        Span::styled("mode ", Style::new().fg(theme.overlay)),
        Span::styled(pb.mode.label(), Style::new().fg(theme.text)),
        Span::styled("   │   ", Style::new().fg(theme.surface1)),
        Span::styled("fmt ", Style::new().fg(theme.overlay)),
        Span::styled(fmt_text, Style::new().fg(theme.text)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
