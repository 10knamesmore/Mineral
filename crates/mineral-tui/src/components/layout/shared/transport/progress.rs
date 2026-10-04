//! 统一绘制普通进度线与波形：轨道按列混入当前背景，强调色只停留在播放头附近。

use mineral_config::ProgressConfig;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::waveform;
use crate::components::frame::FrameEnv;
use crate::components::layout::shared::text::column_bg;
use crate::render::color::lerp_color;
use crate::render::theme::{Theme, permille_of};
use crate::runtime::playback::Playback;

/// 在同一行绘制进度；包络到达时只改变字形和入场提亮，不改变轨道的取色规则。
pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, playback: &Playback, env: FrameEnv<'_>) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let position = TrackPosition::new(playback, area.width);
    let columns = waveform::columns(playback, env.config.tui().waveform(), width);
    for col in 0..area.width {
        let index = usize::from(col);
        let bg = column_bg(frame, area, index);
        let color = color_at(col, bg, &position, env.config.tui().progress(), env.theme);
        let wave = columns.as_ref().and_then(|columns| columns.get(index));
        let (glyph, style) = match wave.and_then(|wave| wave.glyph.map(|glyph| (glyph, wave.glow)))
        {
            Some((glyph, glow)) => (
                glyph,
                Style::new().fg(lerp_color(color, env.theme.text, u64::from(glow), 1000)),
            ),
            None if index < position.filled => ("━", Style::new().fg(color)),
            None if index == position.filled => {
                ("●", Style::new().fg(color).add_modifier(Modifier::BOLD))
            }
            None => ("─", Style::new().fg(color)),
        };
        if let Some(cell) = frame.buffer_mut().cell_mut((area.x + col, area.y)) {
            cell.set_symbol(glyph).set_style(style);
        }
    }
}

/// 当前帧的播放和缓冲边界；播放头保留亚列精度，着色随播放连续移动。
struct TrackPosition {
    /// 轨道总宽，字符列。
    width: usize,

    /// 已完整播放的列数，下一列承载普通进度线的播放头。
    filled: usize,

    /// 已缓冲区间的右开边界，不早于播放头后一列。
    buffered_end: usize,

    /// 播放头位置，以万分之一字符列为单位。
    head_e4: u64,
}

impl TrackPosition {
    /// 将后端比例映射到当前宽度，缓冲落后于播放位置时不产生反向轨道。
    fn new(playback: &Playback, width: u16) -> Self {
        let ratio = playback.ratio_bps();
        let head_e4 = u64::from(ratio.get()) * u64::from(width);
        let width = usize::from(width);
        let filled = ratio.of(width).min(width);
        Self {
            width,
            filled,
            buffered_end: playback.buffered_bps.of(width).max(filled + 1).min(width),
            head_e4,
        }
    }
}

/// 以该列实际背景计算轨道色；播放头前后分别向轨道色和已播色淡出。
fn color_at(
    col: u16,
    bg: Color,
    position: &TrackPosition,
    cfg: &ProgressConfig,
    theme: &Theme,
) -> Color {
    let track = cfg.track();
    let played = theme
        .text_over(bg, permille_of(*track.played_alpha()))
        .unwrap_or(theme.subtext);
    if position.filled >= position.width {
        return played;
    }
    let column_e4 = u64::from(col) * 10_000;
    let head = cfg.playhead();
    let (resting_color, distance, radius) = if column_e4 < position.head_e4 {
        (
            played,
            position.head_e4 - column_e4,
            u64::from(*head.trail_columns()) * 10_000,
        )
    } else {
        let track_color = if usize::from(col) < position.buffered_end {
            theme
                .text_over(bg, permille_of(*track.buffered_alpha()))
                .unwrap_or(theme.surface1)
        } else {
            theme
                .text_over(bg, permille_of(*track.unbuffered_alpha()))
                .unwrap_or(theme.surface0)
        };
        (
            track_color,
            column_e4 - position.head_e4,
            u64::from(*head.lead_columns()) * 10_000,
        )
    };
    let head_color = lerp_color(
        theme.accent,
        theme.text,
        u64::from(permille_of(*head.text_mix())),
        1000,
    );
    if distance == 0 {
        return head_color;
    }
    if distance >= radius {
        return resting_color;
    }
    lerp_color(resting_color, head_color, radius - distance, radius)
}
