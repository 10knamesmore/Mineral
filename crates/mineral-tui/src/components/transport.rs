//! Transport 面板:now-line / 进度条 / 控制按钮 / vol·mode。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::playback::{Playback, PlaybackOrigin, format_ms};
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

/// transport 顶行:居中显示当前曲名(无歌时 `—`)。
fn paint_now(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let title = pb.track.as_ref().map_or("—", |t| t.name.as_str());
    let line = Line::from(title.to_owned())
        .style(Style::new().fg(theme.text).add_modifier(Modifier::BOLD));
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

/// transport 第二行:`artist · album` 居中(灰斜体)。
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

/// 进度条:`elapsed ━━━●─── total`,宽度 < 12 或剩余空间不够时跳过。
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

/// 控件区:`[⏮] [▶/⏸] [⏭] [mode]` 按钮 + 下方对应键位 label,等宽对齐避免抖动。
fn paint_controls(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    if area.height < 2 {
        return;
    }
    let [btn, lbl] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let play_glyph = if pb.playing { "⏸" } else { "▶" };
    let mode_glyph = pb.mode.glyph();
    // (button, label) 槽列表。每个 label 在自己 button 的 cell 宽度内居中,
    // 槽间固定 GAP 空格。两行 join 出来总宽相同,Center alignment 自然对齐。
    // 解决之前 mode_glyph 宽度变化(→/⇄ 1-cell vs ↻∞/↻¹ 2-cell)导致 label 错位。
    let slots: [(String, &str); 4] = [
        ("[⏮]".to_owned(), "p"),
        (format!("[{play_glyph}]"), "␣"),
        ("[⏭]".to_owned(), "n"),
        (format!("[{mode_glyph}]"), "m"),
    ];
    const GAP: usize = 3;
    let gap = " ".repeat(GAP);
    let mut buttons = String::new();
    let mut labels = String::new();
    for (i, (b, l)) in slots.iter().enumerate() {
        if i > 0 {
            buttons.push_str(&gap);
            labels.push_str(&gap);
        }
        let bw = UnicodeWidthStr::width(b.as_str());
        let lw = UnicodeWidthStr::width(*l);
        let pad_total = bw.saturating_sub(lw);
        let lpad = pad_total / 2;
        let rpad = pad_total - lpad;
        buttons.push_str(b);
        labels.push_str(&" ".repeat(lpad));
        labels.push_str(l);
        labels.push_str(&" ".repeat(rpad));
    }
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

/// 右下角:固定宽 10 的音量条 + 当前码率/格式标签。
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
    // 文本用 channel 实测的 format + bitrate;颜色按实测音质分级(见 fmt_tier_color)。
    let (fmt_text, fmt_color) = pb
        .play_url
        .as_ref()
        .map(|pu| {
            let text = format!("{} {}kbps", pu.format, pu.bitrate_bps / 1000);
            (
                text,
                fmt_tier_color(pu.format.is_lossless(), pu.bitrate_bps, theme),
            )
        })
        .unwrap_or_else(|| ("—".to_owned(), theme.overlay));
    // 来源已知 → 来源字形(上色)作 fmt 前缀;未知 → 退回旧的 `fmt` 文字标签。
    let (badge_glyph, badge_color) = pb
        .play_origin
        .map_or(("fmt", theme.overlay), |o| origin_badge(o, theme));
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
        Span::styled(format!("{badge_glyph} "), Style::new().fg(badge_color)),
        Span::styled(fmt_text, Style::new().fg(fmt_color)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// 来源徽标:字形 + 颜色。download=绿(永久在库)/ cache=蓝(LRU 临时)/ remote=灰(网络流)。
///
/// # Params:
///   - `origin`: 当前在播音频的来源
///   - `theme`: 取色主题
///
/// # Return:
///   `(字形, 颜色)`。
fn origin_badge(origin: PlaybackOrigin, theme: &Theme) -> (&'static str, Color) {
    match origin {
        PlaybackOrigin::Download => ("↓", theme.green),
        PlaybackOrigin::Cache => ("◆", theme.accent_2),
        PlaybackOrigin::Remote => ("○", theme.overlay),
    }
}

/// 按 channel **实测**的格式(无损与否)+ 实际码率分 5 档配色。
///
/// 刻意不读 `PlayUrl::quality`——那是请求侧的归一化等级,channel 可「尽力提供」
/// 返回完全不同的实际音质(如 local channel 无视请求)。显示音质必须以实测为准。
fn fmt_tier_color(lossless: bool, bitrate_bps: u32, theme: &Theme) -> Color {
    match (lossless, bitrate_bps) {
        (true, b) if b >= 1_800_000 => theme.yellow, // Hi-Res 级无损(≈24bit/96k 起)
        (true, _) => theme.accent,                   // 无损(FLAC/WAV/APE/ALAC)
        (false, b) if b >= 320_000 => theme.green,   // 高码有损
        (false, b) if b >= 192_000 => theme.text,    // 中码
        _ => theme.overlay,                          // 低码 / 缺失
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use mineral_model::{AudioFormat, BitRate, MediaUrl, PlayUrl};

    use super::fmt_tier_color;
    use crate::playback::{Playback, PlaybackOrigin};
    use crate::test_support::{song, with_duration, with_name};
    use crate::theme::Theme;

    /// 造一个「播放中 + 指定来源 + PlayUrl」的 Playback,供来源徽标快照。
    fn pb_with_origin(origin: PlaybackOrigin, format: AudioFormat, bitrate_bps: u32) -> Playback {
        let track = with_duration(with_name(song("1"), "捕风"), 225_000);
        let song_id = track.id.clone();
        let mut pb = Playback::new();
        pb.track = Some(track);
        pb.position_ms = 60_000;
        pb.playing = true;
        pb.volume_pct = 80;
        pb.play_origin = Some(origin);
        pb.play_url = Some(PlayUrl {
            song_id,
            url: MediaUrl::Local("/x".into()),
            bitrate_bps,
            quality: BitRate::Lossless,
            size: 0,
            format,
        });
        pb
    }

    /// 把档位颜色映射成「音质秩」(越高越好)——同一 lossless 类内,码率↑ 秩不该↓。
    fn tier_rank(c: Color, theme: &Theme) -> u8 {
        match c {
            x if x == theme.yellow => 4, // Hi-Res 级无损
            x if x == theme.accent => 3, // 普通无损
            x if x == theme.green => 2,  // 高码有损
            x if x == theme.text => 1,   // 中码有损
            _ => 0,                      // 低码 / 缺失(overlay)
        }
    }

    proptest! {
        /// 无损只配 `{accent, yellow}`、有损只配 `{green, text, overlay}`——两类配色不串。
        #[test]
        fn prop_tier_color_partitioned(lossless in any::<bool>(), bitrate in 0u32..3_000_000) {
            let theme = Theme::default();
            let c = fmt_tier_color(lossless, bitrate, &theme);
            if lossless {
                prop_assert!(c == theme.accent || c == theme.yellow);
            } else {
                prop_assert!(c == theme.green || c == theme.text || c == theme.overlay);
            }
        }

        /// 同一 lossless 类内,码率单调不降 ⇒ 档位秩单调不降(阈值排序无错位)。
        #[test]
        fn prop_tier_monotonic_in_bitrate(
            lossless in any::<bool>(),
            b1 in 0u32..3_000_000,
            b2 in 0u32..3_000_000,
        ) {
            let (lo, hi) = if b1 <= b2 { (b1, b2) } else { (b2, b1) };
            let theme = Theme::default();
            let r_lo = tier_rank(fmt_tier_color(lossless, lo, &theme), &theme);
            let r_hi = tier_rank(fmt_tier_color(lossless, hi, &theme), &theme);
            prop_assert!(r_lo <= r_hi);
        }
    }

    /// 无 track:transport 空态。
    #[test]
    fn transport_no_track_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(50, 8))?;
        let pb = Playback::new();
        t.draw(|f| super::draw(f, f.area(), &pb, &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:无曲目空态", t.backend());
        Ok(())
    }

    /// 播放中:进度条 + 时间 + 音量(EndSerenading 首曲 LoveLetterTypewriter)。
    #[test]
    fn transport_playing_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(50, 8))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("1"), "LoveLetterTypewriter"),
            225_000,
        ));
        pb.position_ms = 60_000;
        pb.playing = true;
        pb.volume_pct = 80;
        t.draw(|f| super::draw(f, f.area(), &pb, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "播放栏:播放中(LoveLetterTypewriter,进度条 + 音量)",
            t.backend()
        );
        Ok(())
    }

    /// 暂停 + 长歌名(EndSerenading 末曲 TheLastWordIsRejoice,验证长名对齐 / 截断)。
    #[test]
    fn transport_paused_long_title_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(50, 8))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("10"), "TheLastWordIsRejoice"),
            309_000,
        ));
        pb.position_ms = 30_000;
        pb.playing = false;
        t.draw(|f| super::draw(f, f.area(), &pb, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "播放栏:暂停 + 长歌名(TheLastWordIsRejoice)",
            t.backend()
        );
        Ok(())
    }

    /// CJK 长歌名(Chinese Football《地球上最后一个EMO男孩》,中英混排)的宽字符
    /// 居中对齐 / 截断。
    #[test]
    fn transport_cjk_title_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(50, 8))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("c6"), "地球上最后一个EMO男孩"),
            240_000,
        ));
        pb.position_ms = 60_000;
        pb.playing = true;
        t.draw(|f| super::draw(f, f.area(), &pb, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "播放栏:CJK 长歌名(地球上最后一个EMO男孩,中英混排)",
            t.backend()
        );
        Ok(())
    }

    /// 来源徽标的 (字形, 颜色) 映射——文本快照不记颜色,这里把三态钉死。
    #[test]
    fn origin_badge_maps_glyph_and_color() {
        let theme = Theme::default();
        assert_eq!(
            super::origin_badge(PlaybackOrigin::Download, &theme),
            ("↓", theme.green)
        );
        assert_eq!(
            super::origin_badge(PlaybackOrigin::Cache, &theme),
            ("◆", theme.accent_2)
        );
        assert_eq!(
            super::origin_badge(PlaybackOrigin::Remote, &theme),
            ("○", theme.overlay)
        );
    }

    /// 来源徽标:download(↓ 绿,FLAC 999kbps)。
    #[test]
    fn transport_origin_download_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 8))?;
        let pb = pb_with_origin(PlaybackOrigin::Download, AudioFormat::Flac, 999_000);
        t.draw(|f| super::draw(f, f.area(), &pb, &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 download(↓ 绿)", t.backend());
        Ok(())
    }

    /// 来源徽标:cache(◆ 蓝,FLAC 999kbps)。
    #[test]
    fn transport_origin_cache_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 8))?;
        let pb = pb_with_origin(PlaybackOrigin::Cache, AudioFormat::Flac, 999_000);
        t.draw(|f| super::draw(f, f.area(), &pb, &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 cache(◆ 蓝)", t.backend());
        Ok(())
    }

    /// 来源徽标:remote(○ 灰,MP3 320kbps)。
    #[test]
    fn transport_origin_remote_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 8))?;
        let pb = pb_with_origin(PlaybackOrigin::Remote, AudioFormat::Mp3, 320_000);
        t.draw(|f| super::draw(f, f.area(), &pb, &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 remote(○ 灰)", t.backend());
        Ok(())
    }
}
