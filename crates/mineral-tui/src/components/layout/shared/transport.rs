//! Transport 面板:now-line / 进度条 / 控制按钮 / vol·mode。

use mineral_audio::Bps;
use mineral_model::AudioFormat;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::components::layout::shared::marquee::MarqueeCtx;
use crate::components::layout::shared::text::alias_span;
use crate::render::theme::Theme;
use crate::runtime::format::{format_ms, format_ms_opt};
use crate::runtime::marquee::Slot;
use crate::runtime::playback::{Playback, PlaybackOrigin, PrefetchStage};

/// 渲染 Transport 面板到给定 [`Rect`]。
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    pb: &Playback,
    marquee: &MarqueeCtx<'_>,
    theme: &Theme,
) {
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

    paint_now(frame, now, pb, marquee, theme);
    paint_meta(frame, meta, pb, theme);
    paint_progress(frame, prog, pb, theme);
    paint_controls(frame, ctrl, pb, theme);
    paint_vol_mode(frame, vms, pb, theme);
}

/// transport 顶行:居中显示当前曲名(无歌时 `—`),带别名时后缀暗色 ` (alias)`;
/// 溢出按 marquee 相位循环滚动(溢出时切片恰满行宽,居中对齐退化为贴满)。
fn paint_now(
    frame: &mut Frame<'_>,
    area: Rect,
    pb: &Playback,
    marquee: &MarqueeCtx<'_>,
    theme: &Theme,
) {
    if area.height == 0 {
        return;
    }
    let name_style = Style::new().fg(theme.text).add_modifier(Modifier::BOLD);
    let line = match pb.track.as_ref() {
        None => Line::from(Span::styled("—", name_style)),
        Some(t) => {
            let mut spans = vec![Span::styled(t.name.clone(), name_style)];
            spans.extend(alias_span(t.alias.as_deref(), theme));
            marquee.line(spans, Slot::Transport, &t.id.qualified(), area.width)
        }
    };
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
    let total = format_ms_opt(pb.duration_ms());
    // 留 elapsed + 1 + (bar) + 1 + total + 1*2 padding
    let reserve = u16::try_from(elapsed.len() + total.len() + 4).unwrap_or(area.width);
    let bar_w = usize::from(area.width.saturating_sub(reserve));
    if bar_w == 0 {
        return;
    }
    let filled = pb.ratio_bps().of(bar_w);
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
        // 播放头之后的轨道再分两段:已缓冲(亮)+ 未缓冲(暗)。同一 `─` 字形仅靠颜色区分,
        // cell 数守恒,布局不抖。overlay(中灰)比 surface0 明显亮一大档但不抢已播的亮蓝,
        // 形成「已播亮蓝 > 已缓冲中灰 > 未缓冲暗灰」的三级层次,缓冲进度即这段亮轨道的长度。
        let (buffered, unbuffered) = split_buffered_track(bar_w, filled, pb.buffered_bps);
        if buffered > 0 {
            spans.push(Span::styled(
                "─".repeat(buffered),
                Style::new().fg(theme.overlay),
            ));
        }
        if unbuffered > 0 {
            spans.push(Span::styled(
                "─".repeat(unbuffered),
                Style::new().fg(theme.surface0),
            ));
        }
    }
    spans.push(Span::styled(
        format!(" {total} "),
        Style::new().fg(theme.subtext),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 播放头之后的轨道按缓冲进度拆成 `(已缓冲亮段, 未缓冲暗段)` 的 cell 数。
///
/// 缓冲位永远不早于播放头(已播部分必然已缓冲),两段之和恒等于 `bar_w - filled - 1`
/// ——即原本整条未播放轨道的长度,故不改变进度条总宽,布局不抖。
///
/// # Params:
///   - `bar_w`: 进度条总 cell 宽
///   - `filled`: 已播放实心 cell 数,调用方保证 `< bar_w`
///   - `buffered`: 已缓冲比例
///
/// # Return:
///   `(亮段 cell 数, 暗段 cell 数)`,二者之和 = `bar_w - filled - 1`。
fn split_buffered_track(bar_w: usize, filled: usize, buffered: Bps) -> (usize, usize) {
    let track_len = bar_w.saturating_sub(filled).saturating_sub(1);
    let bright = buffered
        .of(bar_w)
        .saturating_sub(filled.saturating_add(1))
        .min(track_len);
    (bright, track_len - bright)
}

/// 控件区:`[⏮] [▶/⏸] [⏭] [mode]` 按钮 + 下方对应键位 label,等宽对齐避免抖动。
/// 下一曲已预排(gapless prefetch)时,`[⏭]` 右侧 GAP 的第一格画 `⇣` 标记
/// (见 [`prefetch_marker`])——占用本就存在的空格 cell,总宽不变、出现/消失不挪动按钮。
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
    let marker = prefetch_marker(pb.prefetch.stage(), theme);
    let mut buttons = Vec::<Span<'_>>::new();
    let mut labels = String::new();
    for (i, (b, l)) in slots.iter().enumerate() {
        if i > 0 {
            // [⏭] 之后(mode 槽之前)的 gap:第一格让给 prefetch 标记,余下补空格;
            // 无标记时整段照旧空格,文本/宽度与标记态完全一致。
            if i == 3
                && let Some((glyph, color)) = marker
            {
                buttons.push(Span::styled(glyph, Style::new().fg(color)));
                buttons.push(Span::raw(" ".repeat(GAP - 1)));
            } else {
                buttons.push(Span::raw(gap.clone()));
            }
            labels.push_str(&gap);
        }
        let bw = UnicodeWidthStr::width(b.as_str());
        let lw = UnicodeWidthStr::width(*l);
        let pad_total = bw.saturating_sub(lw);
        let lpad = pad_total / 2;
        let rpad = pad_total - lpad;
        buttons.push(Span::raw(b.clone()));
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

/// gapless prefetch 标记:字形 + 颜色;未预排 → `None`(不画)。
///
/// 字形恒 `⇣`(1 cell)只换色——拉取中暗(overlay)、就绪亮(green),状态切换布局不抖。
///
/// # Params:
///   - `stage`: 预排阶段(见 [`crate::runtime::playback::Prefetch::stage`])
///   - `theme`: 取色主题
///
/// # Return:
///   `(字形, 颜色)`;`Idle` 为 `None`。
fn prefetch_marker(stage: PrefetchStage, theme: &Theme) -> Option<(&'static str, Color)> {
    match stage {
        PrefetchStage::Idle => None,
        PrefetchStage::Fetching => Some(("⇣", theme.overlay)),
        PrefetchStage::Ready => Some(("⇣", theme.green)),
    }
}

/// vms 行:vol / mode / fmt 三块三等分铺开(分别左 / 居中 / 右对齐)。fmt 段 =
/// format + 位深 + 采样率 + 码率,各缺失即省略(见 [`fmt_spec_label`])。
fn paint_vol_mode(frame: &mut Frame<'_>, area: Rect, pb: &Playback, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let [vol_area, mode_area, fmt_area] = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .areas(area);

    const BAR_W: usize = 10;
    let filled = usize::from(pb.volume_pct) * BAR_W / 100;
    let fill = "█".repeat(filled);
    let empty = "░".repeat(BAR_W.saturating_sub(filled));
    let vol = Line::from(vec![
        Span::styled(" vol ", Style::new().fg(theme.overlay)),
        Span::styled(fill, Style::new().fg(theme.accent)),
        Span::styled(empty, Style::new().fg(theme.surface0)),
        Span::styled(
            format!(" {}%", pb.volume_pct),
            Style::new().fg(theme.subtext),
        ),
    ]);
    frame.render_widget(Paragraph::new(vol).alignment(Alignment::Left), vol_area);

    let mode = Line::from(vec![
        Span::styled("mode ", Style::new().fg(theme.overlay)),
        Span::styled(pb.mode.label(), Style::new().fg(theme.text)),
    ]);
    frame.render_widget(Paragraph::new(mode).alignment(Alignment::Center), mode_area);

    // PlayUrl 在 PlayUrlReady / prefetch 命中后写入,切歌瞬间清 None;没拿到时整段显 `—`。
    // 采样率取 engine 实测(pb.sample_rate_hz),与 format / bitrate(PlayUrl 实测)同为「当前在播」口径。
    let (text, color) = pb
        .play_url
        .as_ref()
        .map(|pu| {
            (
                fmt_spec_label(
                    pu.format.as_ref(),
                    pu.bit_depth,
                    pb.sample_rate_hz,
                    pu.bitrate_bps,
                ),
                fmt_tier_color(
                    pu.format.as_ref().is_some_and(AudioFormat::is_lossless),
                    pu.bitrate_bps,
                    theme,
                ),
            )
        })
        .unwrap_or_else(|| ("—".to_owned(), theme.overlay));
    // 来源已知 → 来源字形(上色)作前缀;未知 → 退回 `fmt` 文字标签。
    let (badge_glyph, badge_color) = pb
        .play_origin
        .map_or(("fmt", theme.overlay), |o| origin_badge(o, theme));
    let fmt = Line::from(vec![
        Span::styled(format!("{badge_glyph} "), Style::new().fg(badge_color)),
        Span::styled(text, Style::new().fg(color)),
        Span::raw(" "),
    ]);
    frame.render_widget(Paragraph::new(fmt).alignment(Alignment::Right), fmt_area);
}

/// 把 channel 实测的 format / 位深 / 采样率 / 码率拼成 fmt 段文本,如 `FLAC 24bit/96kHz 999kbps`。
///
/// 任一项未知即省略对应片段(码率未知不显 `0kbps` 撒谎)——故网易云 mp3 退到
/// `MP3 44.1kHz 320kbps`、刚切歌(采样率未探出)退到 `FLAC 999kbps`;全部未知退 `—`。
///
/// # Params:
///   - `format`: 实测容器格式;`None` = 未知
///   - `bit_depth`: 位深(bit),无损实测有值,否则 `None`
///   - `sample_rate_hz`: engine 实测采样率(Hz),`0` 表示未探出
///   - `bitrate_bps`: 实测码率(bps);`None` = 未知
///
/// # Return:
///   拼好的 fmt 段文本。
fn fmt_spec_label(
    format: Option<&AudioFormat>,
    bit_depth: Option<u8>,
    sample_rate_hz: u32,
    bitrate_bps: Option<u32>,
) -> String {
    let mut parts = Vec::<String>::new();
    if let Some(f) = format {
        parts.push(f.to_string());
    }
    let mut specs = Vec::<String>::new();
    if let Some(bits) = bit_depth {
        specs.push(format!("{bits}bit"));
    }
    if let Some(khz) = fmt_sample_rate(sample_rate_hz) {
        specs.push(khz);
    }
    if !specs.is_empty() {
        parts.push(specs.join("/"));
    }
    if let Some(bps) = bitrate_bps {
        parts.push(format!("{}kbps", bps / 1000));
    }
    if parts.is_empty() {
        "—".to_owned()
    } else {
        parts.join(" ")
    }
}

/// 采样率(Hz)→ 紧凑 kHz 文本:`44100`→`44.1kHz`、`48000`→`48kHz`、`96000`→`96kHz`。
///
/// 整除 1000 显整数、否则留 1 位小数;`0`(未起播 / 未探出)→ `None`(调用方据此省略采样率段)。
///
/// # Params:
///   - `hz`: 采样率(Hz)
///
/// # Return:
///   kHz 文本,`0` 为 `None`。
fn fmt_sample_rate(hz: u32) -> Option<String> {
    if hz == 0 {
        return None;
    }
    let khz = f64::from(hz) / 1000.0;
    if hz.is_multiple_of(1000) {
        Some(format!("{khz:.0}kHz"))
    } else {
        Some(format!("{khz:.1}kHz"))
    }
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
fn fmt_tier_color(lossless: bool, bitrate_bps: Option<u32>, theme: &Theme) -> Color {
    match (lossless, bitrate_bps) {
        (true, Some(b)) if b >= 1_800_000 => theme.yellow, // Hi-Res 级无损(≈24bit/96k 起)
        (true, _) => theme.accent,                         // 无损(FLAC/WAV/APE/ALAC)
        (false, Some(b)) if b >= 320_000 => theme.green,   // 高码有损
        (false, Some(b)) if b >= 192_000 => theme.text,    // 中码
        _ => theme.overlay,                                // 低码 / 码率未知
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use mineral_audio::Bps;
    use mineral_model::{AudioFormat, BitRate, MediaUrl, PlayUrl};

    use super::{fmt_sample_rate, fmt_spec_label, fmt_tier_color, split_buffered_track};
    use crate::components::layout::shared::marquee::MarqueeCtx;
    use crate::render::theme::Theme;
    use crate::runtime::marquee::Marquees;
    use crate::runtime::playback::{Playback, PlaybackOrigin};
    use crate::test_support::{song, with_duration, with_name};

    /// 静止相位(停顿拉满)的 marquee 状态——本组多数测试关注点不在滚动。
    fn still_marquees() -> Marquees {
        Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ u32::MAX)
    }

    /// 测试用 marquee 上下文(gap 取默认配置同款,fade 关)。
    fn ctx(m: &Marquees) -> MarqueeCtx<'_> {
        MarqueeCtx {
            marquees: m,
            gap: "  ✦  ",
            gap_style: ratatui::style::Style::new(),
            fade_to: ratatui::style::Color::Reset,
            fade_cols: 3,
        }
    }

    /// 长曲名溢出:顶行按 marquee 相位滚动——推进拍数后开头滚出、窗口从对应列起。
    #[test]
    fn transport_long_title_marquees() -> color_eyre::Result<()> {
        let mut mq = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        let mut pb = Playback::new();
        pb.track = Some(with_name(
            song("1"),
            "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz",
        ));
        let render = |mq: &Marquees| -> color_eyre::Result<String> {
            let mut t = Terminal::new(TestBackend::new(50, 8))?;
            t.draw(|f| super::draw(f, f.area(), &pb, &ctx(mq), &Theme::default()))?;
            let buf = t.backend().buffer();
            // y=1:边框内顶行(now-line)。
            Ok((0..buf.area.width)
                .filter_map(|x| buf.cell((x, 1)).map(ratatui::buffer::Cell::symbol))
                .collect::<String>())
        };
        let first = render(&mq)?;
        assert!(
            first.trim_start_matches('│').starts_with("abcdef"),
            "建档帧应从开头显示: {first}"
        );
        for _ in 0..3 {
            mq.tick();
        }
        let scrolled = render(&mq)?;
        assert!(
            scrolled.trim_start_matches('│').starts_with("defghi"),
            "推进 3 拍后应从第 4 列字符起显示: {scrolled}"
        );
        Ok(())
    }

    /// 造一个「播放中 + 指定来源 + PlayUrl」的 Playback,供来源徽标快照。
    fn pb_with_origin(
        origin: PlaybackOrigin,
        format: Option<AudioFormat>,
        bitrate_bps: Option<u32>,
        bit_depth: Option<u8>,
    ) -> Playback {
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
            size: None,
            format,
            bit_depth,
            stream_headers: Vec::new(),
            layout: mineral_model::StreamLayout::Contiguous,
            substituted: false,
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
            let c = fmt_tier_color(lossless, Some(bitrate), &theme);
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
            let r_lo = tier_rank(fmt_tier_color(lossless, Some(lo), &theme), &theme);
            let r_hi = tier_rank(fmt_tier_color(lossless, Some(hi), &theme), &theme);
            prop_assert!(r_lo <= r_hi);
        }

        /// 亮段 + 暗段恒等于「播放头之后的轨道长度」(`bar_w - filled - 1`),
        /// 即缓冲 overlay 永不改变进度条总宽——布局不会因缓冲值抖动。
        #[test]
        fn prop_split_track_conserves_width(
            bar_w in 1usize..200,
            filled in 0usize..200,
            bps in 0u16..=10_000,
        ) {
            prop_assume!(filled < bar_w);
            let (bright, dim) = split_buffered_track(bar_w, filled, Bps::new(bps));
            prop_assert_eq!(bright + dim, bar_w - filled - 1);
            // 缓冲比例单调:bps↑ ⇒ 亮段不减。
            let (bright_more, _) = split_buffered_track(bar_w, filled, Bps::new(bps.saturating_add(1)));
            prop_assert!(bright_more >= bright);
        }
    }

    /// `split_buffered_track`:满格全亮 / 缓冲≤已播则无亮段 / 半缓冲分两段。
    #[test]
    fn split_buffered_track_cases() {
        // bar_w=11,filled=2(播放头占 1),轨道 = 11-2-1 = 8 cell。
        // 满缓冲:整条轨道都亮。
        assert_eq!(split_buffered_track(11, 2, Bps::FULL), (8, 0));
        // 零缓冲:全暗(等价改动前行为)。
        assert_eq!(split_buffered_track(11, 2, Bps::ZERO), (0, 8));
        // 缓冲 50% → buffered_cells = 5;亮段 = 5-(2+1)=2,暗段 = 8-2=6。
        assert_eq!(split_buffered_track(11, 2, Bps::new(5_000)), (2, 6));
        // 缓冲落在播放头之内(25% → buffered_cells=2 ≤ filled+1)→ 无亮段。
        assert_eq!(split_buffered_track(11, 2, Bps::new(2_500)), (0, 8));
    }

    /// `fmt_sample_rate`:0→None;整除 1000 显整数、否则留 1 位小数;覆盖常见档位。
    #[test]
    fn fmt_sample_rate_cases() {
        assert_eq!(fmt_sample_rate(0), None);
        assert_eq!(fmt_sample_rate(44_100).as_deref(), Some("44.1kHz"));
        assert_eq!(fmt_sample_rate(48_000).as_deref(), Some("48kHz"));
        assert_eq!(fmt_sample_rate(96_000).as_deref(), Some("96kHz"));
        assert_eq!(fmt_sample_rate(88_200).as_deref(), Some("88.2kHz"));
        assert_eq!(fmt_sample_rate(192_000).as_deref(), Some("192kHz"));
    }

    /// `fmt_spec_label`:任一项缺失即省略对应段(码率未知不显 `0kbps`),format 经 Display 落小写。
    #[test]
    fn fmt_spec_label_cases() {
        // 本地无损:位深 + 采样率全有。
        assert_eq!(
            fmt_spec_label(Some(&AudioFormat::Flac), Some(24), 96_000, Some(999_000)),
            "flac 24bit/96kHz 999kbps"
        );
        // 流式有损:无位深、有采样率。
        assert_eq!(
            fmt_spec_label(Some(&AudioFormat::Mp3), None, 44_100, Some(320_000)),
            "mp3 44.1kHz 320kbps"
        );
        // 刚切歌:采样率未探出(0)且无位深 → 退到 format + 码率。
        assert_eq!(
            fmt_spec_label(Some(&AudioFormat::Flac), None, 0, Some(999_000)),
            "flac 999kbps"
        );
        // 仅位深(采样率未探出):位深段独立成立,不带 `/`。
        assert_eq!(
            fmt_spec_label(Some(&AudioFormat::Flac), Some(16), 0, Some(999_000)),
            "flac 16bit 999kbps"
        );
        // 码率未知(B站 bandwidth 缺失):省略 kbps 段,不显 `0kbps` 撒谎。
        assert_eq!(
            fmt_spec_label(Some(&AudioFormat::Aac), None, 44_100, None),
            "aac 44.1kHz"
        );
        // 全部未知:退 `—` 占位。
        assert_eq!(fmt_spec_label(None, None, 0, None), "—");
    }

    /// fmt 段含位深 + 采样率的完整渲染:本地无损(↓ 绿,flac 24bit/96kHz 999kbps),
    /// 且 vol / mode / fmt 三块三等分铺开。
    #[test]
    fn transport_fmt_bit_hz_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 8))?;
        let mut pb = pb_with_origin(
            PlaybackOrigin::Download,
            Some(AudioFormat::Flac),
            Some(999_000),
            /*bit_depth*/ Some(24),
        );
        pb.sample_rate_hz = 96_000;
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
        crate::test_support::assert_snap!(
            "播放栏:fmt 含位深+采样率(flac 24bit/96kHz)",
            t.backend()
        );
        Ok(())
    }

    /// 无 track:transport 空态。
    #[test]
    fn transport_no_track_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(50, 8))?;
        let pb = Playback::new();
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
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
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
        crate::test_support::assert_snap!(
            "播放栏:播放中(LoveLetterTypewriter,进度条 + 音量)",
            t.backend()
        );
        Ok(())
    }

    /// 曲名带别名:顶行后缀暗色 ` (alias)`,与曲目列表同形式(真实样本 迷星叫 / Mayoiuta)。
    #[test]
    fn transport_alias_suffix_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(50, 8))?;
        let mut pb = Playback::new();
        pb.track = Some(mineral_test::aliased_song());
        pb.playing = true;
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:曲名带别名,后缀暗色 (alias)", t.backend());
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
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
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
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
        crate::test_support::assert_snap!(
            "播放栏:CJK 长歌名(地球上最后一个EMO男孩,中英混排)",
            t.backend()
        );
        Ok(())
    }

    /// prefetch 标记的 (字形, 颜色) 映射——未预排无标记;拉取中暗(overlay)、就绪亮(green),
    /// 字形恒 `⇣` 只换色,布局不抖。
    #[test]
    fn prefetch_marker_maps_glyph_and_color() {
        use crate::runtime::playback::PrefetchStage;
        let theme = Theme::default();
        assert_eq!(super::prefetch_marker(PrefetchStage::Idle, &theme), None);
        assert_eq!(
            super::prefetch_marker(PrefetchStage::Fetching, &theme),
            Some(("⇣", theme.overlay))
        );
        assert_eq!(
            super::prefetch_marker(PrefetchStage::Ready, &theme),
            Some(("⇣", theme.green))
        );
    }

    /// prefetch 标记渲染:next 已预排时 `[⏭]` 右侧 gap 第一格画 `⇣`,按钮行总宽不变。
    #[test]
    fn transport_prefetch_marker_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(50, 8))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("1"), "LoveLetterTypewriter"),
            225_000,
        ));
        pb.position_ms = 220_000; // 曲终临近,prefetch 已触发
        pb.playing = true;
        pb.volume_pct = 80;
        pb.prefetch.ready = true;
        pb.prefetch.buffered_bps = Bps::new(4_000);
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:gapless prefetch 标记(⏭ 右侧 ⇣)", t.backend());
        Ok(())
    }

    /// prefetch 标记颜色随阶段切换:拉取中 overlay → 就绪 green;未预排不画 `⇣`。
    /// 文本快照不记前景色,这里直接读 `⇣` cell 的 fg 钉死。
    #[test]
    fn transport_prefetch_marker_colors() -> color_eyre::Result<()> {
        let theme = Theme::default();
        /// 渲染一帧,取 `⇣` cell 的前景色(无标记则 None)。
        fn marker_fg(pb: &Playback, theme: &Theme) -> color_eyre::Result<Option<Color>> {
            let mut t = Terminal::new(TestBackend::new(50, 8))?;
            let mq = still_marquees();
            t.draw(|f| super::draw(f, f.area(), pb, &ctx(&mq), theme))?;
            Ok(t.backend()
                .buffer()
                .content
                .iter()
                .find(|c| c.symbol() == "⇣")
                .map(|c| c.fg))
        }
        let mut pb = Playback::new();
        pb.track = Some(with_duration(with_name(song("1"), "Prefetching"), 225_000));
        pb.position_ms = 220_000;
        pb.playing = true;
        // 未预排:无标记。
        assert_eq!(marker_fg(&pb, &theme)?, None, "Idle 不该画 ⇣");
        // 已预排、字节未稳:暗色拉取中。
        pb.prefetch.ready = true;
        pb.prefetch.buffered_bps = Bps::new(4_000);
        assert_eq!(
            marker_fg(&pb, &theme)?,
            Some(theme.overlay),
            "Fetching 应 overlay"
        );
        // 字节下完:亮色就绪。
        pb.prefetch.download_complete = true;
        assert_eq!(marker_fg(&pb, &theme)?, Some(theme.green), "Ready 应 green");
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
        let pb = pb_with_origin(
            PlaybackOrigin::Download,
            Some(AudioFormat::Flac),
            Some(999_000),
            /*bit_depth*/ None,
        );
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 download(↓ 绿)", t.backend());
        Ok(())
    }

    /// 来源徽标:cache(◆ 蓝,FLAC 999kbps)。
    #[test]
    fn transport_origin_cache_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 8))?;
        let pb = pb_with_origin(
            PlaybackOrigin::Cache,
            Some(AudioFormat::Flac),
            Some(999_000),
            /*bit_depth*/ None,
        );
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 cache(◆ 蓝)", t.backend());
        Ok(())
    }

    /// 来源徽标:remote(○ 灰,MP3 320kbps)。
    #[test]
    fn transport_origin_remote_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 8))?;
        let pb = pb_with_origin(
            PlaybackOrigin::Remote,
            Some(AudioFormat::Mp3),
            Some(320_000),
            /*bit_depth*/ None,
        );
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &Theme::default()))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 remote(○ 灰)", t.backend());
        Ok(())
    }

    /// 缓冲 overlay 的颜色:播放头之后先一段 overlay(已缓冲,中灰)再一段 surface0(未缓冲,
    /// 暗灰),两色不交错且都非空。文本快照不记前景色,故这里直接读 `cell.fg` 钉死颜色与顺序。
    #[test]
    fn transport_buffer_overlay_colors() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut t = Terminal::new(TestBackend::new(50, 8))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(with_name(song("1"), "Buffering"), 225_000));
        pb.position_ms = 60_000; // ≈26.7% 已播
        pb.playing = true;
        pb.buffered_bps = Bps::new(6_000); // 60% 已缓冲:介于已播与满之间 → 亮暗两段都该出现
        let mq = still_marquees();
        t.draw(|f| super::draw(f, f.area(), &pb, &ctx(&mq), &theme))?;

        // 圆角边框的上下边也是 `─` 且同为 surface1,故不能全局扫字形。先用唯一的填充字符
        // `━` 定位进度条所在行,只在该行内取 `─` 轨道,避开边框。
        let buf = t.backend().buffer();
        let w = usize::from(buf.area.width);
        let prog_row = buf
            .content
            .iter()
            .enumerate()
            .find(|(_, c)| c.symbol() == "━")
            .map(|(i, _)| i / w)
            .ok_or_else(|| color_eyre::eyre::eyre!("未找到进度条行(无 ━ 填充)"))?;
        let track: Vec<Color> = buf
            .content
            .iter()
            .enumerate()
            .filter(|(i, c)| i / w == prog_row && c.symbol() == "─")
            .map(|(_, c)| c.fg)
            .collect();

        let bright = track.iter().filter(|c| **c == theme.overlay).count();
        let dim = track.iter().filter(|c| **c == theme.surface0).count();
        assert!(bright > 0, "应有已缓冲亮段(overlay):{track:?}");
        assert!(dim > 0, "缓冲未满应有未缓冲暗段(surface0):{track:?}");
        assert_eq!(
            bright + dim,
            track.len(),
            "轨道只该是 surface0/overlay 两色"
        );

        // 亮段必须全部排在暗段之前——缓冲连续紧随播放头,不交错。
        let last_bright = track.iter().rposition(|c| *c == theme.overlay);
        let first_dim = track.iter().position(|c| *c == theme.surface0);
        if let (Some(lb), Some(fd)) = (last_bright, first_dim) {
            assert!(lb < fd, "亮段应全部在暗段之前:{track:?}");
        }
        Ok(())
    }
}
