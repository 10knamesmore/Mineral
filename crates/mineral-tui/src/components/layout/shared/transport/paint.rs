//! 五行 Transport 面板:上边框状态 / 曲名 / 元数据 / 进度 / 下边框时间与控件。

use super::feedback::{ButtonAppearance, ControlButton, Heading, TransportFeedback};

use mineral_audio::Bps;
use mineral_model::AudioFormat;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::components::layout::shared::marquee::MarqueeCtx;
use crate::components::layout::shared::text::{alias_span, center_bg, char_width, display_width};
use crate::components::layout::shared::waveform::{PlayState, WaveformCtx, waveform_spans};
use crate::render::color::lerp_color;
use crate::render::control_press::background as button_background;
use crate::render::theme::{Ink, Theme};
use crate::runtime::format::{format_ms, format_ms_opt};
use crate::runtime::marquee::Slot;
use crate::runtime::playback::{Playback, PlaybackOrigin, PrefetchStage};

/// 渲染 Transport 面板到给定 [`Rect`]。
pub(crate) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    pb: &Playback,
    feedback: &TransportFeedback,
    marquee: &MarqueeCtx<'_>,
    wave: &WaveformCtx<'_>,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // 弱化色阶按实际背景现算:氛围场上贴场色保持成比例对比;无人铺 bg 的面采到
    // Reset 按 base 混合(text_alpha 不依赖氛围背景),ANSI 主题回落静态 token。
    let ink = theme.ink_over(center_bg(frame, area));
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ink.faint));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    paint_heading(frame, area, pb, feedback, theme, ink);
    if area.height >= 2 {
        let control_bounds = paint_footer(frame, area, pb, feedback, theme, ink);
        paint_controls(frame, area, pb, feedback, control_bounds, theme, ink);
    }

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let [now, meta, prog, _filler] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    paint_now(frame, now, pb, marquee, theme, ink);
    paint_meta(frame, meta, pb, ink);
    paint_progress(frame, prog, pb, wave, theme, ink);
}

/// 按终端列宽裁切，留得下一列时用省略号提示；不切断宽字符。
fn clip_label(text: &str, limit: u16) -> String {
    if display_width(text) <= limit {
        return text.to_owned();
    }
    let mut clipped = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = char_width(ch);
        if used + width > limit.saturating_sub(1) {
            break;
        }
        clipped.push(ch);
        used += width;
    }
    if limit > 0 {
        clipped.push('…');
    }
    clipped
}

/// 两端信息共用边框内宽：音质靠右，标题只占左侧剩余列，不覆盖角或对方文字。
fn paint_heading(
    frame: &mut Frame<'_>,
    area: Rect,
    pb: &Playback,
    feedback: &TransportFeedback,
    theme: &Theme,
    ink: Ink,
) {
    let available = area.width.saturating_sub(2);
    if available == 0 {
        return;
    }
    let (heading, opacity) = feedback.heading();
    let heading_text = match heading {
        Heading::Transport => " transport ".to_owned(),
        Heading::Volume => format!(" vol {:>3}% ", pb.volume_pct),
    };
    let (badge, badge_color) = pb.play_origin.map_or(("fmt", ink.muted), |origin| {
        origin_badge(origin, theme, ink)
    });
    let (spec, spec_color) = media_spec(pb, theme, ink);
    let badge_width = display_width(badge);
    let minimum_right = badge_width + 1;
    let heading_width =
        display_width(" transport ").min(available.saturating_sub(minimum_right + 1));
    if heading_width > 0 {
        let heading_area = Rect::new(area.x.saturating_add(1), area.y, heading_width, 1);
        let bg = fade_background(center_bg(frame, heading_area), theme);
        frame.render_widget(
            Paragraph::new(clip_label(&heading_text, heading_width))
                .style(Style::new().fg(lerp_color(bg, theme.text, u64::from(opacity), 1000))),
            heading_area,
        );
    }

    let gap = u16::from(heading_width > 0);
    let right_width = available.saturating_sub(heading_width + gap);
    if right_width < badge_width {
        return;
    }
    let trailing = u16::from(right_width > badge_width + 1);
    let spec_width = right_width.saturating_sub(badge_width + 1 + trailing);
    let right = Line::from(vec![
        Span::styled(badge, Style::new().fg(badge_color)),
        Span::raw(" "),
        Span::styled(clip_label(&spec, spec_width), Style::new().fg(spec_color)),
        Span::raw(" ".repeat(usize::from(trailing))),
    ]);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        Rect::new(area.right() - 1 - right_width, area.y, right_width, 1),
    );
}

/// 真彩背景直接混合，终端默认或 ANSI 背景沿用主题 base 的既有假设。
fn fade_background(sampled: Color, theme: &Theme) -> Color {
    if matches!(sampled, Color::Rgb(..)) {
        sampled
    } else {
        theme.base
    }
}

/// transport 顶行:居中显示当前曲名(无歌时 `—`),带别名时后缀暗色 ` (alias)`;
/// 溢出按 marquee 相位循环滚动(溢出时切片恰满行宽,居中对齐退化为贴满)。
fn paint_now(
    frame: &mut Frame<'_>,
    area: Rect,
    pb: &Playback,
    marquee: &MarqueeCtx<'_>,
    theme: &Theme,
    ink: Ink,
) {
    if area.height == 0 {
        return;
    }
    let name_style = Style::new().fg(theme.text).add_modifier(Modifier::BOLD);
    let line = match pb.track.as_ref() {
        None => Line::from(Span::styled("—", name_style)),
        Some(t) => {
            let mut spans = vec![Span::styled(t.name.clone(), name_style)];
            spans.extend(alias_span(t.alias.as_deref(), ink.muted));
            marquee.line(spans, Slot::Transport, &t.id.qualified(), area.width)
        }
    };
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

/// transport 第二行:`artist · album` 居中(弱化斜体)。
fn paint_meta(frame: &mut Frame<'_>, area: Rect, pb: &Playback, ink: Ink) {
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
    let line = Line::from(clip_label(&format!("{artist} · {album}"), area.width))
        .style(Style::new().fg(ink.strong).add_modifier(Modifier::ITALIC));
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

/// 进度条占满面板内宽；波形开启且当前曲包络就绪时，轨道原地化身振幅波形。
fn paint_progress(
    frame: &mut Frame<'_>,
    area: Rect,
    pb: &Playback,
    wave: &WaveformCtx<'_>,
    theme: &Theme,
    ink: Ink,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let bar_w = usize::from(area.width);
    let filled = pb.ratio_bps().of(bar_w);
    if wave.enabled
        && let Some(envelope) = wave.envelope
    {
        frame.render_widget(
            Paragraph::new(Line::from(waveform_spans(
                envelope,
                bar_w,
                PlayState {
                    // 连续比例:软边混色要亚列精度,量化在 waveform 内部做
                    progress: pb.ratio_bps(),
                    buffered: pb.buffered_bps,
                },
                wave,
                theme,
                ink,
            ))),
            area,
        );
        return;
    }
    let fill = "━".repeat(filled);
    let mut spans = vec![Span::styled(fill, Style::new().fg(theme.accent_2))];
    if filled < bar_w {
        spans.push(Span::styled(
            "●",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
        // 播放头之后的轨道再分两段:已缓冲(亮)+ 未缓冲(暗)。同一 `─` 字形仅靠颜色区分,
        // cell 数守恒,布局不抖。muted 档比 ghost 档明显亮一大档但不抢已播的亮蓝,
        // 形成「已播亮蓝 > 已缓冲中灰 > 未缓冲暗灰」的三级层次,缓冲进度即这段亮轨道的长度。
        let (buffered, unbuffered) = split_buffered_track(bar_w, filled, pb.buffered_bps);
        if buffered > 0 {
            spans.push(Span::styled(
                "─".repeat(buffered),
                Style::new().fg(ink.muted),
            ));
        }
        if unbuffered > 0 {
            spans.push(Span::styled(
                "─".repeat(unbuffered),
                Style::new().fg(ink.ghost),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 底边框时间和预取状态独立于控件显隐；返回控件可用的中间列区间。
fn paint_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    pb: &Playback,
    feedback: &TransportFeedback,
    theme: &Theme,
    ink: Ink,
) -> Option<(u16, u16)> {
    let elapsed = format!(" {} ", format_ms(pb.position_ms));
    let total = format!(" {} ", format_ms_opt(pb.duration_ms()));
    let left_width = u16::try_from(elapsed.len()).ok()?;
    let right_width = u16::try_from(total.len()).ok()?;
    let inner_width = area.width.saturating_sub(2);
    if inner_width < left_width + right_width + 1 {
        return None;
    }
    let bottom = area.bottom() - 1;
    let right_start = area.width - 1 - right_width;
    let elapsed_area = Rect::new(area.x + 1, bottom, left_width, 1);
    let mut elapsed_style = Style::new().fg(theme.accent);
    let press_strength = feedback.elapsed_press_strength();
    if press_strength > 0 {
        elapsed_style = elapsed_style.bg(button_background(
            center_bg(frame, elapsed_area),
            press_strength,
            theme,
        ));
    }
    frame.render_widget(Paragraph::new(elapsed).style(elapsed_style), elapsed_area);
    frame.render_widget(
        Paragraph::new(total).style(Style::new().fg(ink.strong)),
        Rect::new(area.x + right_start, bottom, right_width, 1),
    );
    let mut left_limit = 1 + left_width + 1;
    let right_limit = right_start.saturating_sub(1);
    if let Some((glyph, color)) = prefetch_marker(pb.prefetch.stage(), theme, ink) {
        let play_start = ((area.width - 1) / 2).saturating_sub(1);
        let width = display_width(glyph) + 1;
        // 图标和尾随空格不占用居中播放键；两侧导航键按剩余空间显示。
        if play_start.saturating_sub(left_limit) >= width {
            frame.render_widget(
                Paragraph::new(format!("{glyph} ")).style(Style::new().fg(color)),
                Rect::new(area.x + left_limit, bottom, width, 1),
            );
            left_limit += width;
        }
    }
    Some((left_limit, right_limit))
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
pub(crate) fn split_buffered_track(bar_w: usize, filled: usize, buffered: Bps) -> (usize, usize) {
    let track_len = bar_w.saturating_sub(filled).saturating_sub(1);
    let bright = buffered
        .of(bar_w)
        .saturating_sub(filled.saturating_add(1))
        .min(track_len);
    (bright, track_len - bright)
}

/// 四个按钮占下边框的局部列；播放图标始终落在面板几何中心附近。
/// 中间相位先让原边框退到背景，再让整组按钮从同一背景浮现。
fn paint_controls(
    frame: &mut Frame<'_>,
    area: Rect,
    pb: &Playback,
    feedback: &TransportFeedback,
    control_bounds: Option<(u16, u16)>,
    theme: &Theme,
    ink: Ink,
) {
    let opacity = feedback.controls_opacity();
    if !feedback.has_visible_buttons() || area.width < 3 {
        return;
    }
    let center = (area.width - 1) / 2;
    let bottom = area.bottom() - 1;
    let right_corner = area.width - 1;
    let (left_limit, right_limit) = control_bounds.unwrap_or((1, right_corner));
    let play_glyph = if pb.playing { "⏸" } else { "▶" };
    if center == 0 || center >= right_corner {
        return;
    }
    // 极窄面板只保留播放状态，不把括号或相邻按钮压在角上。
    if center <= 1
        || center + 1 >= right_corner
        || center - 1 < left_limit
        || center + 2 > right_limit
    {
        if center < left_limit || center >= right_limit {
            return;
        }
        paint_button(
            frame,
            Rect::new(area.x + center, bottom, 1, 1),
            Line::from(play_glyph),
            feedback.button(ControlButton::PlayPause),
            ink,
            theme,
        );
        return;
    }
    let play_start = center - 1;
    if play_start > left_limit {
        paint_gap(
            frame,
            Rect::new(area.x + play_start - 1, bottom, 1, 1),
            opacity,
            ink,
            theme,
        );
    }
    if play_start + 3 < right_limit {
        paint_gap(
            frame,
            Rect::new(area.x + play_start + 3, bottom, 1, 1),
            opacity,
            ink,
            theme,
        );
    }
    paint_button(
        frame,
        Rect::new(area.x + play_start, bottom, 3, 1),
        Line::from(format!("[{play_glyph}]")),
        feedback.button(ControlButton::PlayPause),
        ink,
        theme,
    );

    let gap = if area.width >= 60 {
        3
    } else if area.width >= 48 {
        2
    } else {
        1
    };
    let Some(prev_start) = play_start.checked_sub(3 + gap) else {
        return;
    };
    let next_start = play_start + 3 + gap;
    if prev_start < left_limit || next_start + 3 > right_limit {
        return;
    }
    if prev_start > left_limit {
        paint_gap(
            frame,
            Rect::new(area.x + prev_start - 1, bottom, 1, 1),
            opacity,
            ink,
            theme,
        );
    }
    paint_gap(
        frame,
        Rect::new(area.x + prev_start + 3, bottom, gap, 1),
        opacity,
        ink,
        theme,
    );
    paint_gap(
        frame,
        Rect::new(area.x + play_start + 3, bottom, gap, 1),
        opacity,
        ink,
        theme,
    );
    for (start, label, button) in [
        (prev_start, "[⏮]", ControlButton::Previous),
        (next_start, "[⏭]", ControlButton::Next),
    ] {
        paint_button(
            frame,
            Rect::new(area.x + start, bottom, 3, 1),
            Line::from(Span::styled(label, Style::new().fg(ink.strong))),
            feedback.button(button),
            ink,
            theme,
        );
    }
    let mode_start = next_start + 3 + gap;
    if next_start + 3 < right_limit {
        paint_gap(
            frame,
            Rect::new(
                area.x + next_start + 3,
                bottom,
                gap.min(right_limit - (next_start + 3)),
                1,
            ),
            opacity,
            ink,
            theme,
        );
    }

    let room = right_limit.saturating_sub(mode_start);
    if room < 3 {
        return;
    }
    let (caption, reveal_progress, content_columns) = feedback.mode_caption();
    let glyph = caption.mode.glyph();
    let glyph_columns = display_width(glyph);
    let columns = content_columns.min(room - 2);
    if columns < glyph_columns {
        return;
    }
    let mode_area = Rect::new(area.x + mode_start, bottom, columns + 2, 1);
    if mode_start + mode_area.width < right_limit {
        paint_gap(
            frame,
            Rect::new(mode_area.right(), bottom, 1, 1),
            opacity,
            ink,
            theme,
        );
    }
    let appearance = feedback.button(ControlButton::Mode);
    let bg = button_background(
        center_bg(frame, mode_area),
        appearance.press_strength,
        theme,
    );
    let mut spans = vec![Span::raw("["), Span::raw(glyph)];
    let mut used = glyph_columns;
    if caption.expanded && used < columns {
        spans.push(Span::raw(" "));
        used += 1;
        // 宽度动画只裁去未露出的列，不用省略号替换正在显现的文字。
        for (column, ch) in (0u16..).zip(caption.label().chars()) {
            if used >= columns {
                break;
            }
            let alpha = caption.label_opacity(column, reveal_progress);
            spans.push(Span::styled(
                ch.to_string(),
                Style::new().fg(lerp_color(bg, theme.text, u64::from(alpha), 1000)),
            ));
            used += 1;
        }
    }
    spans.push(Span::raw(" ".repeat(usize::from(columns - used))));
    spans.push(Span::raw("]"));
    paint_button(frame, mode_area, Line::from(spans), appearance, ink, theme);
}

/// 空白间隔与按钮共享显隐相位，覆盖底线但不改变整体宽度。
fn paint_gap(frame: &mut Frame<'_>, area: Rect, opacity: u16, ink: Ink, theme: &Theme) {
    let bg = fade_background(center_bg(frame, area), theme);
    paint_button(
        frame,
        area,
        Line::from(Span::styled(
            " ".repeat(usize::from(area.width)),
            Style::new().fg(bg),
        )),
        ButtonAppearance {
            opacity,
            press_strength: 0,
        },
        ink,
        theme,
    );
}

/// 只覆盖按钮自己的列；按压底色随本键脉冲变化，静止时不覆盖原背景。
fn paint_button(
    frame: &mut Frame<'_>,
    area: Rect,
    line: Line<'_>,
    appearance: ButtonAppearance,
    ink: Ink,
    theme: &Theme,
) {
    let sampled_bg = center_bg(frame, area);
    let bg = fade_background(sampled_bg, theme);
    let opacity = appearance.opacity;
    let line = if opacity <= 500 {
        Line::from(Span::styled(
            "─".repeat(usize::from(area.width)),
            Style::new().fg(lerp_color(ink.faint, bg, u64::from(opacity) * 2, 1000)),
        ))
    } else {
        Line::from(
            line.spans
                .into_iter()
                .map(|span| {
                    let fg = span.style.fg.unwrap_or(theme.text);
                    Span::styled(
                        span.content,
                        span.style
                            .fg(lerp_color(bg, fg, u64::from(opacity - 500) * 2, 1000)),
                    )
                })
                .collect::<Vec<_>>(),
        )
    };
    let mut paragraph = Paragraph::new(line);
    if appearance.press_strength > 0 && opacity > 500 {
        let pressed_bg = button_background(sampled_bg, appearance.press_strength, theme);
        paragraph = paragraph.style(Style::new().bg(lerp_color(
            bg,
            pressed_bg,
            u64::from(opacity - 500) * 2,
            1000,
        )));
    }
    frame.render_widget(paragraph, area);
}

/// gapless prefetch 标记:字形 + 颜色;未预排 → `None`(不画)。
///
/// 拉取中用弱化的 `⇣`，就绪用绿色 `✓`；两个图标都占一列。
///
/// # Params:
///   - `stage`: 预排阶段(见 [`crate::runtime::playback::Prefetch::stage`])
///   - `theme`: 取色主题
///   - `ink`: 对实际背景现算的弱化色阶
///
/// # Return:
///   `(字形, 颜色)`;`Idle` 为 `None`。
fn prefetch_marker(stage: PrefetchStage, theme: &Theme, ink: Ink) -> Option<(&'static str, Color)> {
    match stage {
        PrefetchStage::Idle => None,
        PrefetchStage::Fetching => Some(("⇣", ink.muted)),
        PrefetchStage::Ready => Some(("✓", theme.green)),
    }
}

/// 当前 decoder input 的实测音质文字与颜色；open 前展示未知值。
fn media_spec(pb: &Playback, theme: &Theme, ink: Ink) -> (String, Color) {
    pb.media_info
        .as_ref()
        .map(|info| {
            (
                fmt_spec_label(
                    info.format.as_ref(),
                    info.bit_depth,
                    pb.sample_rate_hz,
                    info.bitrate_bps,
                ),
                fmt_tier_color(
                    info.format.as_ref().is_some_and(AudioFormat::is_lossless),
                    info.bitrate_bps,
                    theme,
                    ink,
                ),
            )
        })
        .unwrap_or_else(|| ("—".to_owned(), ink.muted))
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
///   - `ink`: 对实际背景现算的弱化色阶
///
/// # Return:
///   `(字形, 颜色)`。
fn origin_badge(origin: PlaybackOrigin, theme: &Theme, ink: Ink) -> (&'static str, Color) {
    match origin {
        PlaybackOrigin::Download => ("↓", theme.green),
        PlaybackOrigin::Cache => ("◆", theme.accent_2),
        PlaybackOrigin::Remote => ("○", ink.muted),
    }
}

/// 按 channel **实测**的格式(无损与否)+ 实际码率分 5 档配色。
///
/// 刻意不读 `PlaybackMediaInfo::quality`——那是请求侧的归一化等级,channel 可「尽力提供」
/// 返回完全不同的实际音质(如 local channel 无视请求)。显示音质必须以实测为准。
fn fmt_tier_color(lossless: bool, bitrate_bps: Option<u32>, theme: &Theme, ink: Ink) -> Color {
    match (lossless, bitrate_bps) {
        (true, Some(b)) if b >= 1_800_000 => theme.yellow, // Hi-Res 级无损(≈24bit/96k 起)
        (true, _) => theme.accent,                         // 无损(FLAC/WAV/APE/ALAC)
        (false, Some(b)) if b >= 320_000 => theme.green,   // 高码有损
        (false, Some(b)) if b >= 192_000 => theme.text,    // 中码
        _ => ink.muted,                                    // 低码 / 码率未知
    }
}

#[cfg(test)]
mod tests {
    use proptest::{prop_assert, prop_assert_eq, prop_assume, proptest};

    use mineral_audio::Bps;

    use super::split_buffered_track;

    proptest! {
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
}
