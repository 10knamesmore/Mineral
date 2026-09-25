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
    use std::time::{Duration, Instant};

    use mineral_protocol::PlayMode;

    use proptest::prelude::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::{Frame, Terminal};

    use mineral_audio::Bps;
    use mineral_model::{AudioFormat, BitRate, PlaybackMediaInfo};

    use super::{
        Heading, TransportFeedback, fmt_sample_rate, fmt_spec_label, fmt_tier_color,
        split_buffered_track,
    };
    use crate::components::layout::shared::marquee::MarqueeCtx;
    use crate::components::layout::shared::waveform::{PlayedStyle, RevealStyle, WaveformCtx};
    use crate::render::color::lerp_color;
    use crate::render::theme::Theme;
    use crate::runtime::action::{Action, VolumeDelta};
    use crate::runtime::marquee::Marquees;
    use crate::runtime::playback::{EnvelopeState, Playback, PlaybackOrigin};
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

    /// 测试默认安静状态；需要按钮显现的断言显式传入已 tick 的反馈。
    #[allow(
        clippy::expect_used,
        reason = "测试 fixture 的内置配置加载失败必须终止测试"
    )]
    fn draw_still(
        frame: &mut Frame<'_>,
        pb: &Playback,
        marquees: &Marquees,
        wave: &WaveformCtx<'_>,
        theme: &Theme,
    ) {
        let cfg = mineral_config::Config::defaults().expect("默认配置应能加载");
        let feedback = TransportFeedback::new(pb.mode, cfg.tui().animation());
        super::draw(
            frame,
            frame.area(),
            pb,
            &feedback,
            &ctx(marquees),
            wave,
            theme,
        );
    }

    /// 控制动作后推进到完全显现，供预加载与按钮几何测试使用。
    fn shown_controls(pb: &Playback) -> color_eyre::Result<TransportFeedback> {
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        let now = Instant::now();
        let mut feedback = TransportFeedback::new(pb.mode, anim);
        feedback.on_action(Action::NextSong, pb.mode, anim, now);
        for _ in 0..1000 {
            if feedback.controls_opacity() == 1000 {
                break;
            }
            feedback.tick(pb.mode, anim, now);
        }
        assert_eq!(feedback.controls_opacity(), 1000);
        Ok(feedback)
    }

    /// 长曲名溢出:顶行按 marquee 相位滚动——推进拍数后开头滚出、窗口从对应列起。
    #[test]
    fn transport_long_title_marquees() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut mq = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        let mut pb = Playback::new();
        pb.track = Some(with_name(
            song("1"),
            "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz",
        ));
        let render = |mq: &Marquees| -> color_eyre::Result<String> {
            let mut t = Terminal::new(TestBackend::new(50, 5))?;
            t.draw(|f| draw_still(f, &pb, mq, &WaveformCtx::off(), &theme))?;
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

    /// 造一个带媒体事实与指定来源的 Playback,供来源徽标快照。
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
        pb.media_info = Some(PlaybackMediaInfo {
            song_id,
            bitrate_bps,
            quality: BitRate::Lossless,
            size: None,
            format,
            bit_depth,
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
            let theme = crate::test_support::default_theme().map_err(|e| {
                proptest::test_runner::TestCaseError::fail(format!("加载默认主题失败: {e}"))
            })?;
            let ink = theme.ink_over(ratatui::style::Color::Reset);
            let c = fmt_tier_color(lossless, Some(bitrate), &theme, ink);
            if lossless {
                prop_assert!(c == theme.accent || c == theme.yellow);
            } else {
                prop_assert!(c == theme.green || c == theme.text || c == ink.muted);
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
            let theme = crate::test_support::default_theme().map_err(|e| {
                proptest::test_runner::TestCaseError::fail(format!("加载默认主题失败: {e}"))
            })?;
            let ink = theme.ink_over(ratatui::style::Color::Reset);
            let r_lo = tier_rank(fmt_tier_color(lossless, Some(lo), &theme, ink), &theme);
            let r_hi = tier_rank(fmt_tier_color(lossless, Some(hi), &theme, ink), &theme);
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

    /// fmt 段含位深 + 采样率的完整渲染:右上角本地无损(↓ 绿,flac 24bit/96kHz 999kbps)。
    #[test]
    fn transport_fmt_bit_hz_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(64, 5))?;
        let mut pb = pb_with_origin(
            PlaybackOrigin::Download,
            Some(AudioFormat::Flac),
            Some(999_000),
            /*bit_depth*/ Some(24),
        );
        pb.sample_rate_hz = 96_000;
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
        crate::test_support::assert_snap!(
            "播放栏:fmt 含位深+采样率(flac 24bit/96kHz)",
            t.backend()
        );
        Ok(())
    }

    /// 无 track:transport 空态。
    #[test]
    fn transport_no_track_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(50, 5))?;
        let pb = Playback::new();
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
        crate::test_support::assert_snap!("播放栏:无曲目空态", t.backend());
        Ok(())
    }

    /// 播放中:曲名 + 进度条 + 时间(EndSerenading 首曲 LoveLetterTypewriter)。
    #[test]
    fn transport_playing_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(50, 5))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("1"), "LoveLetterTypewriter"),
            225_000,
        ));
        pb.position_ms = 60_000;
        pb.playing = true;
        pb.volume_pct = 80;
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
        crate::test_support::assert_snap!(
            "播放栏:播放中(LoveLetterTypewriter,进度条)",
            t.backend()
        );
        Ok(())
    }

    /// 曲名带别名:顶行后缀暗色 ` (alias)`,与曲目列表同形式(真实样本 迷星叫 / Mayoiuta)。
    #[test]
    fn transport_alias_suffix_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(50, 5))?;
        let mut pb = Playback::new();
        pb.track = Some(mineral_test::aliased_song());
        pb.playing = true;
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
        crate::test_support::assert_snap!("播放栏:曲名带别名,后缀暗色 (alias)", t.backend());
        Ok(())
    }

    /// 暂停 + 长歌名(EndSerenading 末曲 TheLastWordIsRejoice,验证长名对齐 / 截断)。
    #[test]
    fn transport_paused_long_title_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(50, 5))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("10"), "TheLastWordIsRejoice"),
            309_000,
        ));
        pb.position_ms = 30_000;
        pb.playing = false;
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
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
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(50, 5))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("c6"), "地球上最后一个EMO男孩"),
            240_000,
        ));
        pb.position_ms = 60_000;
        pb.playing = true;
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
        crate::test_support::assert_snap!(
            "播放栏:CJK 长歌名(地球上最后一个EMO男孩,中英混排)",
            t.backend()
        );
        Ok(())
    }

    /// 波形进度条:开关开 + 包络就绪(归属当前曲)→ 轨道段化身振幅块字符,
    /// 时间文本与占位完全不变。
    #[test]
    fn transport_waveform_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 5))?;
        let theme = crate::test_support::default_theme()?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("1"), "CrescendoTrack"),
            225_000,
        ));
        pb.position_ms = 60_000;
        pb.playing = true;
        pb.volume_pct = 80;
        let track_id = pb
            .track
            .as_ref()
            .map(|s| s.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("track 必在"))?;
        // 合成渐强包络:波形应从矮到高有可见起伏。
        let points = (0..200u16)
            .map(|i| u8::try_from((u32::from(i) * 255) / 199).unwrap_or(255))
            .collect::<Vec<u8>>();
        // 揭示满值:本快照锁的是稳态形态,入场动画中途另有专门快照。
        pb.envelope = Some(EnvelopeState::test_at(
            track_id,
            mineral_model::Envelope { points, version: 1 },
            /*reveal_e3*/ 1000,
        ));
        let wave = WaveformCtx {
            enabled: true,
            played: PlayedStyle::Solid(theme.accent_2),
            // 线性基线:快照锁重采样/字形/着色,gamma 语义由 waveform 纯函数测试锁。
            contrast: 1.0,
            edge_radius: 3,
            reveal: RevealStyle {
                sweep: 620,
                glow: 850,
            },
            envelope: pb.current_envelope(),
        };
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &wave, &theme))?;
        crate::test_support::assert_snap!("播放栏:波形进度条(渐强包络化身块字符)", t.backend());
        Ok(())
    }

    /// 回落语义:开关开但包络缺失时,渲染与关闭态逐 cell 完全一致(普通进度条)。
    #[test]
    fn waveform_without_envelope_falls_back_to_plain_bar() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(with_name(song("1"), "NoEnvelope"), 225_000));
        pb.position_ms = 60_000;
        pb.playing = true;
        let render = |wave: &WaveformCtx<'_>| -> color_eyre::Result<ratatui::buffer::Buffer> {
            let mut t = Terminal::new(TestBackend::new(64, 5))?;
            let mq = still_marquees();
            t.draw(|f| draw_still(f, &pb, &mq, wave, &theme))?;
            Ok(t.backend().buffer().clone())
        };
        let on_without_envelope = WaveformCtx {
            enabled: true,
            played: PlayedStyle::Solid(theme.accent_2),
            contrast: 2.0,
            edge_radius: 3,
            reveal: RevealStyle {
                sweep: 620,
                glow: 850,
            },
            envelope: None,
        };
        assert_eq!(
            render(&on_without_envelope)?,
            render(&WaveformCtx::off())?,
            "包络缺失时开关开与关必须逐 cell 一致"
        );
        Ok(())
    }

    /// 入场动画第一帧(揭示为 0)必须与波形**关闭态逐 cell 完全一致**——包络到达那一刻
    /// 画面不跳变,动画从既有的普通进度条形态无缝接续长出波形。
    #[test]
    fn reveal_first_frame_matches_plain_bar() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(with_name(song("1"), "RevealStart"), 225_000));
        pb.position_ms = 60_000;
        pb.playing = true;
        pb.buffered_bps = Bps::new(6_000);
        let track_id = pb
            .track
            .as_ref()
            .map(|s| s.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("track 必在"))?;
        pb.envelope = Some(EnvelopeState::test_at(
            track_id,
            mineral_model::Envelope {
                points: (0..200u16)
                    .map(|i| u8::try_from(i % 256).unwrap_or(0))
                    .collect(),
                version: 1,
            },
            /*reveal_e3*/ 0,
        ));
        let render = |wave: &WaveformCtx<'_>| -> color_eyre::Result<ratatui::buffer::Buffer> {
            let mut t = Terminal::new(TestBackend::new(64, 5))?;
            let mq = still_marquees();
            t.draw(|f| draw_still(f, &pb, &mq, wave, &theme))?;
            Ok(t.backend().buffer().clone())
        };
        let starting = WaveformCtx {
            enabled: true,
            played: PlayedStyle::Solid(theme.accent_2),
            contrast: 2.0,
            edge_radius: 3,
            reveal: RevealStyle {
                sweep: 620,
                glow: 850,
            },
            envelope: pb.current_envelope(),
        };
        assert_eq!(
            render(&starting)?,
            render(&WaveformCtx::off())?,
            "揭示第一帧必须与普通进度条逐 cell 一致"
        );
        Ok(())
    }

    /// 入场动画中途(揭示 45%):左段已长成波形、揭示边前沿提亮、右段仍是进度条中线,
    /// 三段共存于同一行且总宽不变。
    #[test]
    fn transport_waveform_reveal_midway_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 5))?;
        let theme = crate::test_support::default_theme()?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(
            with_name(song("1"), "CrescendoTrack"),
            225_000,
        ));
        pb.position_ms = 60_000;
        pb.playing = true;
        pb.volume_pct = 80;
        let track_id = pb
            .track
            .as_ref()
            .map(|s| s.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("track 必在"))?;
        let points = (0..200u16)
            .map(|i| u8::try_from((u32::from(i) * 255) / 199).unwrap_or(255))
            .collect::<Vec<u8>>();
        pb.envelope = Some(EnvelopeState::test_at(
            track_id,
            mineral_model::Envelope { points, version: 1 },
            /*reveal_e3*/ 450,
        ));
        let wave = WaveformCtx {
            enabled: true,
            played: PlayedStyle::Solid(theme.accent_2),
            contrast: 1.0,
            edge_radius: 3,
            reveal: RevealStyle {
                sweep: 620,
                glow: 850,
            },
            envelope: pb.current_envelope(),
        };
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &wave, &theme))?;
        crate::test_support::assert_snap!(
            "播放栏:波形入场动画中途(左波形 / 亮边 / 右中线三段共存)",
            t.backend()
        );
        Ok(())
    }

    /// 预取状态同时通过字形和颜色区分，未预排时留白。
    #[test]
    fn prefetch_marker_maps_glyph_and_color() -> color_eyre::Result<()> {
        use crate::runtime::playback::PrefetchStage;
        let theme = crate::test_support::default_theme()?;
        let ink = theme.ink_over(Color::Reset);
        assert_eq!(
            super::prefetch_marker(PrefetchStage::Idle, &theme, ink),
            None
        );
        assert_eq!(
            super::prefetch_marker(PrefetchStage::Fetching, &theme, ink),
            Some(("⇣", ink.muted))
        );
        assert_eq!(
            super::prefetch_marker(PrefetchStage::Ready, &theme, ink),
            Some(("✓", theme.green))
        );
        Ok(())
    }

    /// 预取信息位于已播放时间之后，和中间控制键共存。
    #[test]
    fn transport_prefetch_marker_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(50, 5))?;
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
        let feedback = shown_controls(&pb)?;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &pb,
                &feedback,
                &ctx(&mq),
                &WaveformCtx::off(),
                &theme,
            )
        })?;
        crate::test_support::assert_snap!("播放栏:已播放时间后的下一首预取状态", t.backend());
        Ok(())
    }

    /// 控件隐藏时仍显示预取阶段，后端清除预排后才移除；字形与颜色同步改变。
    #[test]
    fn transport_prefetch_marker_colors() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        /// 控件隐藏时读取预取标记的字形与颜色。
        fn marker(pb: &Playback, theme: &Theme) -> color_eyre::Result<Option<(String, Color)>> {
            let mut t = Terminal::new(TestBackend::new(50, 5))?;
            let mq = still_marquees();
            t.draw(|f| draw_still(f, pb, &mq, &WaveformCtx::off(), theme))?;
            Ok(t.backend()
                .buffer()
                .content
                .iter()
                .find(|c| matches!(c.symbol(), "⇣" | "✓"))
                .map(|c| (c.symbol().to_owned(), c.fg)))
        }
        let mut pb = Playback::new();
        pb.track = Some(with_duration(with_name(song("1"), "Prefetching"), 225_000));
        pb.position_ms = 220_000;
        pb.playing = true;
        // 未预排:无标记。
        assert_eq!(marker(&pb, &theme)?, None);
        // 已预排、字节未稳:暗色拉取中。
        pb.prefetch.ready = true;
        pb.prefetch.buffered_bps = Bps::new(4_000);
        assert_eq!(
            marker(&pb, &theme)?,
            Some(("⇣".to_owned(), theme.ink_over(Color::Reset).muted))
        );
        // producer 下载完成后缓冲满值:亮色就绪。
        pb.prefetch.buffered_bps = Bps::FULL;
        assert_eq!(marker(&pb, &theme)?, Some(("✓".to_owned(), theme.green)));
        pb.prefetch.ready = false;
        assert_eq!(marker(&pb, &theme)?, None);
        Ok(())
    }

    /// 来源徽标的 (字形, 颜色) 映射——文本快照不记颜色,这里把三态钉死。
    #[test]
    fn origin_badge_maps_glyph_and_color() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let ink = theme.ink_over(Color::Reset);
        assert_eq!(
            super::origin_badge(PlaybackOrigin::Download, &theme, ink),
            ("↓", theme.green)
        );
        assert_eq!(
            super::origin_badge(PlaybackOrigin::Cache, &theme, ink),
            ("◆", theme.accent_2)
        );
        assert_eq!(
            super::origin_badge(PlaybackOrigin::Remote, &theme, ink),
            ("○", ink.muted)
        );
        Ok(())
    }

    /// 氛围背景(真彩 bg)下，transport 弱化色阶按实际背景混合；蓝底与红底得到不同前景色。
    #[test]
    fn transport_ink_follows_actual_bg() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let probe = |bg: Color| -> color_eyre::Result<Color> {
            let mut t = Terminal::new(TestBackend::new(50, 5))?;
            let mut pb = Playback::new();
            pb.track = Some(with_duration(with_name(song("1"), "InkProbe"), 225_000));
            let mq = still_marquees();
            t.draw(|f| {
                let area = f.area();
                f.render_widget(
                    ratatui::widgets::Block::new().style(ratatui::style::Style::new().bg(bg)),
                    area,
                );
                draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme);
            })?;
            let cell = t
                .backend()
                .buffer()
                .cell(ratatui::layout::Position::new(0, 0))
                .ok_or_else(|| color_eyre::eyre::eyre!("边框角 cell 应存在"))?;
            Ok(cell.fg)
        };
        let on_blue = probe(Color::Rgb(0, 0, 120))?;
        let on_red = probe(Color::Rgb(120, 0, 0))?;
        assert_ne!(on_blue, on_red, "faint 档应跟随实际背景");
        let (Color::Rgb(br, _, bb), Color::Rgb(rr, _, rb)) = (on_blue, on_red) else {
            color_eyre::eyre::bail!("探针应拿到真彩 fg: {on_blue:?} / {on_red:?}");
        };
        assert!(bb > br, "蓝底边框:蓝分量占优,got {on_blue:?}");
        assert!(rr > rb, "红底边框:红分量占优,got {on_red:?}");
        Ok(())
    }

    /// 来源徽标:download(↓ 绿,FLAC 999kbps)。
    #[test]
    fn transport_origin_download_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(64, 5))?;
        let pb = pb_with_origin(
            PlaybackOrigin::Download,
            Some(AudioFormat::Flac),
            Some(999_000),
            /*bit_depth*/ None,
        );
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 download(↓ 绿)", t.backend());
        Ok(())
    }

    /// 来源徽标:cache(◆ 蓝,FLAC 999kbps)。
    #[test]
    fn transport_origin_cache_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(64, 5))?;
        let pb = pb_with_origin(
            PlaybackOrigin::Cache,
            Some(AudioFormat::Flac),
            Some(999_000),
            /*bit_depth*/ None,
        );
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 cache(◆ 蓝)", t.backend());
        Ok(())
    }

    /// 来源徽标:remote(○ 灰,MP3 320kbps)。
    #[test]
    fn transport_origin_remote_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(64, 5))?;
        let pb = pb_with_origin(
            PlaybackOrigin::Remote,
            Some(AudioFormat::Mp3),
            Some(320_000),
            /*bit_depth*/ None,
        );
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;
        crate::test_support::assert_snap!("播放栏:来源徽标 remote(○ 灰)", t.backend());
        Ok(())
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .filter_map(|x| buffer.cell((x, y)).map(ratatui::buffer::Cell::symbol))
            .collect::<String>()
    }

    /// 宽度足够时四个按钮完整展开；窄面板保留居中播放键，模式文案可裁切。
    #[test]
    fn controls_stay_centered_with_expanded_modes() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        for width in [40u16, 41, 59, 60, 61, 99, 100] {
            for mode in [
                PlayMode::Sequential,
                PlayMode::RepeatAll,
                PlayMode::RepeatOne,
                PlayMode::Shuffle,
            ] {
                let mut pb = Playback::new();
                pb.mode = mode;
                pb.playing = true;
                pb.prefetch.ready = true;
                pb.prefetch.buffered_bps = Bps::FULL;
                let now = Instant::now();
                let mut feedback = TransportFeedback::new(mode, anim);
                feedback.on_action(Action::CyclePlayMode, mode, anim, now);
                for _ in 0..1000 {
                    feedback.tick(mode, anim, now);
                }
                let mut t = Terminal::new(TestBackend::new(width, 5))?;
                let mq = still_marquees();
                t.draw(|f| {
                    super::draw(
                        f,
                        f.area(),
                        &pb,
                        &feedback,
                        &ctx(&mq),
                        &WaveformCtx::off(),
                        &theme,
                    )
                })?;
                let buffer = t.backend().buffer();
                let center = (width - 1) / 2;
                assert_eq!(
                    buffer.cell((center, 4)).map(ratatui::buffer::Cell::symbol),
                    Some("⏸"),
                    "width={width}, mode={mode:?}"
                );
                let prev =
                    (0..width).find(|&x| buffer.cell((x, 4)).is_some_and(|c| c.symbol() == "⏮"));
                let next =
                    (0..width).find(|&x| buffer.cell((x, 4)).is_some_and(|c| c.symbol() == "⏭"));
                let (Some(prev), Some(next)) = (prev, next) else {
                    color_eyre::eyre::bail!("两侧按钮不完整: {}", row(buffer, 4));
                };
                assert_eq!(
                    center - prev,
                    next - center,
                    "两侧按钮须对称: {}",
                    row(buffer, 4)
                );
                let expected = format!("[{} {}]", mode.glyph(), feedback.mode_caption().0.label());
                if width >= 59 {
                    assert!(
                        row(buffer, 4).contains(&expected),
                        "width={width} 缺 {expected}: {}",
                        row(buffer, 4)
                    );
                } else {
                    assert!(row(buffer, 4).contains(mode.glyph()));
                }
            }
        }
        Ok(())
    }

    /// 模式切换不让图标变暗，文字从左到右显现，伸缩过程中不插入省略号。
    #[test]
    fn mode_reveal_keeps_the_confirmed_icon_visible() -> color_eyre::Result<()> {
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        let theme = crate::test_support::default_theme()?;
        let now = Instant::now();
        let mut pb = Playback::new();
        pb.mode = PlayMode::RepeatAll;
        let mut feedback = TransportFeedback::new(pb.mode, anim);
        feedback.on_action(Action::CyclePlayMode, pb.mode, anim, now);
        for _ in 0..80 {
            feedback.tick(pb.mode, anim, now);
        }
        pb.mode = PlayMode::RepeatOne;
        feedback.sync_mode(pb.mode, anim);
        let mut terminal = Terminal::new(TestBackend::new(64, 5))?;
        let mq = still_marquees();
        let mut saw_left_to_right_reveal = false;
        for _ in 0..20 {
            terminal.draw(|frame| {
                super::draw(
                    frame,
                    frame.area(),
                    &pb,
                    &feedback,
                    &ctx(&mq),
                    &WaveformCtx::off(),
                    &theme,
                );
            })?;
            let buffer = terminal.backend().buffer();
            let icon_start = (0..64)
                .find(|&x| buffer[(x, 4)].symbol() == "↻")
                .ok_or_else(|| color_eyre::eyre::eyre!("当前模式图标必须可见"))?;
            assert_eq!(buffer[(icon_start + 1, 4)].symbol(), "¹");
            assert_eq!(buffer[(icon_start, 4)].fg, theme.text);
            assert_eq!(buffer[(icon_start + 1, 4)].fg, theme.text);
            let first = buffer[(icon_start + 3, 4)].fg;
            let last = buffer[(icon_start + 9, 4)].fg;
            if let (Color::Rgb(left, _, _), Color::Rgb(right, _, _)) = (first, last) {
                saw_left_to_right_reveal |= left > right;
            }
            assert!(!row(buffer, 4).contains('…'));
            feedback.tick(pb.mode, anim, now);
        }
        assert!(saw_left_to_right_reveal);
        pb.mode = PlayMode::Shuffle;
        feedback.sync_mode(pb.mode, anim);
        for _ in 0..20 {
            terminal.draw(|frame| {
                super::draw(
                    frame,
                    frame.area(),
                    &pb,
                    &feedback,
                    &ctx(&mq),
                    &WaveformCtx::off(),
                    &theme,
                );
            })?;
            assert!(!row(terminal.backend().buffer(), 4).contains('…'));
            feedback.tick(pb.mode, anim, now);
        }
        Ok(())
    }

    /// 左上标题双向交接时，右上音质的字形、位置和颜色保持不变。
    #[test]
    fn volume_heading_does_not_move_quality() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        let now = Instant::now();
        let mut pb = pb_with_origin(
            PlaybackOrigin::Cache,
            Some(AudioFormat::Flac),
            Some(2_304_000),
            Some(24),
        );
        pb.sample_rate_hz = 96_000;
        let mq = still_marquees();
        for width in [40u16, 60, 100] {
            let mut feedback = TransportFeedback::new(pb.mode, anim);
            let mut terminal = Terminal::new(TestBackend::new(width, 5))?;
            terminal.draw(|f| {
                super::draw(
                    f,
                    f.area(),
                    &pb,
                    &feedback,
                    &ctx(&mq),
                    &WaveformCtx::off(),
                    &theme,
                )
            })?;
            let idle = terminal.backend().buffer().clone();
            feedback.on_action(Action::NudgeVolume(VolumeDelta(5)), pb.mode, anim, now);
            for at in [
                now,
                now + Duration::from_millis(u64::from(*anim.transport().volume_hold_ms())),
            ] {
                for _ in 0..40 {
                    feedback.tick(pb.mode, anim, at);
                    terminal.draw(|f| {
                        super::draw(
                            f,
                            f.area(),
                            &pb,
                            &feedback,
                            &ctx(&mq),
                            &WaveformCtx::off(),
                            &theme,
                        )
                    })?;
                    for x in 12..width {
                        assert_eq!(
                            terminal.backend().buffer().cell((x, 0)),
                            idle.cell((x, 0)),
                            "右上信息不能随着标题改变，width={width}, x={x}"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// 进度轨道占满内宽；底边框两端时间与中间按钮在窄宽度下不互盖。
    #[test]
    fn progress_and_bottom_times_fit_at_narrow_widths() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(song("1"), 225_000));
        pb.position_ms = 60_000;
        pb.playing = true;
        pb.prefetch.ready = true;
        pb.prefetch.buffered_bps = Bps::FULL;
        let feedback = shown_controls(&pb)?;
        let mq = still_marquees();
        for width in [15u16, 16, 20, 24, 30, 40, 50, 64] {
            let mut terminal = Terminal::new(TestBackend::new(width, 5))?;
            terminal.draw(|frame| {
                super::draw(
                    frame,
                    frame.area(),
                    &pb,
                    &feedback,
                    &ctx(&mq),
                    &WaveformCtx::off(),
                    &theme,
                )
            })?;
            let buffer = terminal.backend().buffer();
            let progress = row(buffer, 3);
            let bottom = row(buffer, 4);
            assert_eq!(buffer.cell((1, 3)).map(Cell::symbol), Some("━"));
            assert_eq!(buffer.cell((width - 2, 3)).map(Cell::symbol), Some("─"));
            assert!(bottom.starts_with("╰ 1:00 "), "width={width}: {bottom}");
            assert!(bottom.ends_with(" 3:45 ╯"), "width={width}: {bottom}");
            if width >= 20 {
                assert_eq!(
                    buffer.cell(((width - 1) / 2, 4)).map(Cell::symbol),
                    Some("⏸"),
                    "width={width}: {bottom}"
                );
            }
            if width >= 24 {
                let marker = (0..width)
                    .find(|&x| buffer[(x, 4)].symbol() == "✓")
                    .ok_or_else(|| color_eyre::eyre::eyre!("width={width}: 缺少预取状态"))?;
                assert!(marker > 6 && marker < (width - 1) / 2);
                assert_eq!(buffer[(marker, 4)].fg, theme.green);
            }
            assert!(!progress.contains("1:00") && !progress.contains("3:45"));
        }
        Ok(())
    }

    /// 预取位置和颜色不随控制键淡入淡出，退场后时间与预取状态仍保留。
    #[test]
    fn bottom_border_restores_after_controls_expire() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        let now = Instant::now();
        let mut pb = Playback::new();
        pb.prefetch.ready = true;
        pb.prefetch.buffered_bps = Bps::FULL;
        let mq = still_marquees();
        let mut feedback = TransportFeedback::new(pb.mode, anim);
        let mut t = Terminal::new(TestBackend::new(40, 5))?;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &pb,
                &feedback,
                &ctx(&mq),
                &WaveformCtx::off(),
                &theme,
            )
        })?;
        let idle = row(t.backend().buffer(), 4);
        let marker_x = (0..40)
            .find(|&x| t.backend().buffer()[(x, 4)].symbol() == "✓")
            .ok_or_else(|| color_eyre::eyre::eyre!("控件隐藏时也应显示预取状态"))?;
        let marker_cell = t.backend().buffer()[(marker_x, 4)].clone();

        feedback.on_action(Action::NextSong, pb.mode, anim, now);
        let expired =
            now + Duration::from_millis(u64::from(*anim.transport().controls_hold_ms()) + 1);
        for at in [now, expired] {
            for _ in 0..40 {
                feedback.tick(pb.mode, anim, at);
                t.draw(|f| {
                    super::draw(
                        f,
                        f.area(),
                        &pb,
                        &feedback,
                        &ctx(&mq),
                        &WaveformCtx::off(),
                        &theme,
                    )
                })?;
                assert_eq!(t.backend().buffer()[(marker_x, 4)], marker_cell);
            }
        }
        assert_eq!(feedback.controls_opacity(), 0);
        assert_eq!(row(t.backend().buffer(), 4), idle);
        Ok(())
    }

    /// 实际背景蓝 / 红不同，中间帧在标题原位混色；无背景时按 theme.base 混合。
    #[test]
    fn heading_fade_samples_local_background_and_confirmed_volume() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        let now = Instant::now();
        let mut pb = Playback::new();
        pb.volume_pct = 80;
        let mut feedback = TransportFeedback::new(pb.mode, anim);
        feedback.on_action(Action::NudgeVolume(VolumeDelta(5)), pb.mode, anim, now);
        feedback.tick(pb.mode, anim, now);
        let (heading, alpha) = feedback.heading();
        assert_eq!(heading, Heading::Transport);
        assert!(alpha > 0 && alpha < 1000);
        let mq = still_marquees();
        for bg in [Color::Rgb(0, 0, 120), Color::Rgb(120, 0, 0), Color::Reset] {
            let mut t = Terminal::new(TestBackend::new(40, 5))?;
            t.draw(|f| {
                f.render_widget(
                    ratatui::widgets::Block::new().style(ratatui::style::Style::new().bg(bg)),
                    f.area(),
                );
                super::draw(
                    f,
                    f.area(),
                    &pb,
                    &feedback,
                    &ctx(&mq),
                    &WaveformCtx::off(),
                    &theme,
                );
            })?;
            let blended_bg = if bg == Color::Reset { theme.base } else { bg };
            let expected = lerp_color(blended_bg, theme.text, u64::from(alpha), 1000);
            assert_eq!(
                t.backend().buffer().cell((2, 0)).map(|c| c.fg),
                Some(expected),
                "bg={bg:?}"
            );
        }
        for _ in 0..1000 {
            feedback.tick(pb.mode, anim, now);
        }
        let mut t = Terminal::new(TestBackend::new(40, 5))?;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &pb,
                &feedback,
                &ctx(&mq),
                &WaveformCtx::off(),
                &theme,
            )
        })?;
        assert!(row(t.backend().buffer(), 0).contains("vol  80%"));
        feedback.tick(
            pb.mode,
            anim,
            now + Duration::from_millis(u64::from(*anim.transport().volume_hold_ms()) + 1),
        );
        for _ in 0..1000 {
            feedback.tick(
                pb.mode,
                anim,
                now + Duration::from_millis(u64::from(*anim.transport().volume_hold_ms()) + 1),
            );
        }
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &pb,
                &feedback,
                &ctx(&mq),
                &WaveformCtx::off(),
                &theme,
            )
        })?;
        assert!(row(t.backend().buffer(), 0).contains("transport"));
        Ok(())
    }

    /// 40 列音质可截断但保留 origin + format；更小或零面积也不得越过给定区域。
    #[test]
    fn transport_narrow_areas_clip_without_overwriting_neighbors() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut pb = pb_with_origin(
            PlaybackOrigin::Download,
            Some(AudioFormat::Flac),
            Some(999_000),
            Some(24),
        );
        pb.sample_rate_hz = 96_000;
        pb.prefetch.ready = true;
        pb.prefetch.buffered_bps = Bps::FULL;
        let feedback = shown_controls(&pb)?;
        let mq = still_marquees();
        for width in [40u16, 60, 100] {
            let mut t = Terminal::new(TestBackend::new(width, 5))?;
            t.draw(|f| {
                super::draw(
                    f,
                    f.area(),
                    &pb,
                    &feedback,
                    &ctx(&mq),
                    &WaveformCtx::off(),
                    &theme,
                )
            })?;
            let top = row(t.backend().buffer(), 0);
            assert!(
                top.contains("transport") && top.contains("↓ flac"),
                "width={width}: {top}"
            );
            if width >= 60 {
                assert!(top.contains("999kbps"), "width={width}: {top}");
            } else {
                assert!(top.contains('…'), "40 列音质裁切应提示: {top}");
            }
        }
        let mut t = Terminal::new(TestBackend::new(44, 6))?;
        for width in [0u16, 1, 2, 3, 6, 10, 14, 22, 30, 39] {
            for height in [0u16, 1, 2, 3, 4, 5] {
                let area = Rect::new(2, 0, width, height);
                t.draw(|f| {
                    super::draw(
                        f,
                        area,
                        &pb,
                        &feedback,
                        &ctx(&mq),
                        &WaveformCtx::off(),
                        &theme,
                    )
                })?;
                for y in 0..6u16 {
                    for x in 0..44u16 {
                        if (x < area.left() || x >= area.right() || y >= area.bottom())
                            && let Some(cell) = t.backend().buffer().cell((x, y))
                        {
                            assert_eq!(cell.symbol(), " ", "outside area: {area:?}, ({x},{y})");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// 首次按下只铺本键底色，脉冲结束后恢复实际背景；播放图标仍是确认态。
    #[test]
    fn button_press_is_local_and_restores_the_background() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        let now = Instant::now();
        let pb = Playback::new();
        let mq = still_marquees();
        for bg in [Color::Reset, Color::Rgb(20, 40, 80)] {
            for (action, glyph) in [
                (Action::PrevOrRestart, "⏮"),
                (Action::TogglePlayPause, "▶"),
                (Action::NextSong, "⏭"),
                (Action::CyclePlayMode, "→"),
            ] {
                let mut feedback = TransportFeedback::new(pb.mode, anim);
                let mut terminal = Terminal::new(TestBackend::new(64, 5))?;
                let draw = |terminal: &mut Terminal<TestBackend>, feedback: &TransportFeedback| {
                    terminal
                        .draw(|frame| {
                            frame.render_widget(
                                ratatui::widgets::Block::new()
                                    .style(ratatui::style::Style::new().bg(bg)),
                                frame.area(),
                            );
                            super::draw(
                                frame,
                                frame.area(),
                                &pb,
                                feedback,
                                &ctx(&mq),
                                &WaveformCtx::off(),
                                &theme,
                            );
                        })
                        .map(|_| ())
                };
                draw(&mut terminal, &feedback)?;
                let idle = terminal.backend().buffer().clone();
                feedback.on_action(action, pb.mode, anim, now);
                draw(&mut terminal, &feedback)?;
                let buffer = terminal.backend().buffer();
                let pressed_x = (0..64)
                    .find(|&x| buffer[(x, 4)].symbol() == glyph)
                    .ok_or_else(|| color_eyre::eyre::eyre!("按中的按钮应立即显示"))?;
                for y in 0..5 {
                    for x in 0..64 {
                        if y == 4 && (pressed_x - 1..=pressed_x + 1).contains(&x) {
                            assert_eq!(buffer[(x, y)].bg, theme.surface1);
                        } else {
                            assert_eq!(buffer[(x, y)], idle[(x, y)]);
                        }
                    }
                }
                for _ in 0..6 {
                    feedback.tick(pb.mode, anim, now);
                }
                draw(&mut terminal, &feedback)?;
                let fading_bg = terminal.backend().buffer()[(pressed_x, 4)].bg;
                assert_ne!(fading_bg, theme.surface1);
                assert_ne!(fading_bg, bg);
                for _ in 0..40 {
                    feedback.tick(pb.mode, anim, now);
                }
                draw(&mut terminal, &feedback)?;
                assert_eq!(terminal.backend().buffer()[(pressed_x, 4)].bg, bg);
                assert_eq!(terminal.backend().buffer()[(31, 4)].symbol(), "▶");
            }
        }
        Ok(())
    }

    /// 模式文字的零亮度要融入按压底色，不能留下旧背景色的暗字。
    #[test]
    fn mode_reveal_blends_into_pressed_background() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        let now = Instant::now();
        let mut pb = Playback::new();
        let mut feedback = TransportFeedback::new(pb.mode, anim);
        feedback.on_action(Action::CyclePlayMode, pb.mode, anim, now);
        for _ in 0..80 {
            feedback.tick(pb.mode, anim, now);
        }
        feedback.on_action(Action::CyclePlayMode, pb.mode, anim, now);
        pb.mode = PlayMode::RepeatOne;
        feedback.sync_mode(pb.mode, anim);
        let mq = still_marquees();
        let mut terminal = Terminal::new(TestBackend::new(64, 5))?;
        terminal.draw(|frame| {
            super::draw(
                frame,
                frame.area(),
                &pb,
                &feedback,
                &ctx(&mq),
                &WaveformCtx::off(),
                &theme,
            );
        })?;
        let buffer = terminal.backend().buffer();
        let glyph_x = (0..64)
            .find(|&x| buffer[(x, 4)].symbol() == "↻")
            .ok_or_else(|| color_eyre::eyre::eyre!("确认模式图标应可见"))?;
        let first_letter = &buffer[(glyph_x + 3, 4)];
        assert_eq!(first_letter.bg, theme.surface1);
        assert_eq!(first_letter.fg, first_letter.bg);
        Ok(())
    }

    /// 未按中的按钮仍以背景作桥梁：淡走旧边框后才绘新按钮，不发生字符跳色。
    #[test]
    fn controls_midframe_colors_cross_at_background() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let cfg = mineral_config::Config::defaults()?;
        let anim = cfg.tui().animation();
        let now = Instant::now();
        let pb = Playback::new();
        let mq = still_marquees();
        let mut feedback = TransportFeedback::new(pb.mode, anim);
        feedback.on_action(Action::NextSong, pb.mode, anim, now);
        let mut saw_border = false;
        let mut saw_button = false;
        for _ in 0..1000 {
            feedback.tick(pb.mode, anim, now);
            let alpha = feedback.controls_opacity();
            if alpha == 0 || alpha == 1000 {
                continue;
            }
            let mut t = Terminal::new(TestBackend::new(40, 5))?;
            t.draw(|f| {
                super::draw(
                    f,
                    f.area(),
                    &pb,
                    &feedback,
                    &ctx(&mq),
                    &WaveformCtx::off(),
                    &theme,
                )
            })?;
            let cell = t
                .backend()
                .buffer()
                .cell((18, 4))
                .ok_or_else(|| color_eyre::eyre::eyre!("播放左括号格缺失"))?;
            let ink = theme.ink_over(Color::Reset);
            if alpha <= 500 {
                saw_border = true;
                assert_eq!(cell.symbol(), "─");
                assert_eq!(
                    cell.fg,
                    lerp_color(ink.faint, theme.base, u64::from(alpha) * 2, 1000)
                );
            } else {
                saw_button = true;
                assert_eq!(cell.symbol(), "[");
                assert_eq!(
                    cell.fg,
                    lerp_color(theme.base, theme.text, u64::from(alpha - 500) * 2, 1000)
                );
            }
        }
        assert!(saw_border && saw_button, "显现须经过边框退场和按钮入场两段");
        Ok(())
    }

    /// 缓冲轨道的颜色:播放头之后先一段 muted 档(已缓冲,中灰)再一段 ghost 档(未缓冲,
    /// 近底),两色不交错且都非空。文本快照不记前景色,故这里直接读 `cell.fg` 钉死颜色与顺序。
    #[test]
    fn transport_buffer_overlay_colors() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let ink = theme.ink_over(Color::Reset);
        let mut t = Terminal::new(TestBackend::new(50, 5))?;
        let mut pb = Playback::new();
        pb.track = Some(with_duration(with_name(song("1"), "Buffering"), 225_000));
        pb.position_ms = 60_000; // ≈26.7% 已播
        pb.playing = true;
        pb.buffered_bps = Bps::new(6_000); // 60% 已缓冲:介于已播与满之间 → 亮暗两段都该出现
        let mq = still_marquees();
        t.draw(|f| draw_still(f, &pb, &mq, &WaveformCtx::off(), &theme))?;

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

        let bright = track.iter().filter(|c| **c == ink.muted).count();
        let dim = track.iter().filter(|c| **c == ink.ghost).count();
        assert!(bright > 0, "应有已缓冲亮段(muted 档):{track:?}");
        assert!(dim > 0, "缓冲未满应有未缓冲暗段(ghost 档):{track:?}");
        assert_eq!(bright + dim, track.len(), "轨道只该是 muted/ghost 两色");

        // 亮段必须全部排在暗段之前——缓冲连续紧随播放头,不交错。
        let last_bright = track.iter().rposition(|c| *c == ink.muted);
        let first_dim = track.iter().position(|c| *c == ink.ghost);
        if let (Some(lb), Some(fd)) = (last_bright, first_dim) {
            assert!(lb < fd, "亮段应全部在暗段之前:{track:?}");
        }
        Ok(())
    }
}
