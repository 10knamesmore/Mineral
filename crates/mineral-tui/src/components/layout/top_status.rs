//! 顶部状态行(1 行,无边框):左侧 tabs + 右侧 playback state。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use mineral_audio::AudioBackend;
use mineral_task::ChannelFetchKindTag;

use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, View};

/// 渲染状态行到给定 [`Rect`]。`queue_open` 由浮层栈给出,决定是否显示 `[queue]` tab。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(60)]).areas(area);
    paint_left(frame, left, state, theme);
    paint_right(frame, right, state, theme);
    dim_unfocused(frame, area, state, theme);
}

/// 终端失焦时整行前景向背景色渐变(满进度混 [`UNFOCUS_BLEND_PERMILLE`]),
/// 渲染后整行后处理,与「画什么」解耦。聚焦稳态(进度 0)零开销跳过。
fn dim_unfocused(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let t = state.focus_fade.eased_in_out();
    if t == 0 {
        return;
    }
    let num = u64::from(t) * UNFOCUS_BLEND_PERMILLE;
    let buf = frame.buffer_mut();
    for pos in area.positions() {
        let Some(cell) = buf.cell_mut(pos) else {
            continue;
        };
        // 只动 RGB 前景:非 RGB(Reset / ANSI named)没有可插值的通道,lerp 的
        // 二态降级会让文字在中点直接跳成背景色消失,不如保持原样。
        if matches!(cell.fg, ratatui::style::Color::Rgb(..)) {
            cell.fg = lerp_color(cell.fg, theme.base, num, /*denom*/ 1_000_000);
        }
    }
}

/// 失焦变灰满进度时前景向背景的混合比例(千分比)。不到 1000:全灰会把顶栏
/// 信息抹没,留约一半可读性、视觉上是「整体退后」。
const UNFOCUS_BLEND_PERMILLE: u64 = 550;

/// 左侧:`mineral vX` + `[playlists]` / `[tracks]` tabs。
fn paint_left(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let active_pl = state.view == View::Playlists;
    let active_lib = state.view == View::Library;
    let spans = vec![
        Span::styled(
            format!("▌ mineral v{}  ", env!("CARGO_PKG_VERSION")),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("│  ", Style::new().fg(theme.surface1)),
        Span::styled("[playlists]", tab_style(active_pl, theme)),
        Span::raw("  "),
        Span::styled("[tracks]", tab_style(active_lib, theme)),
    ];
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
    // 失焦徽标:随 focus_fade 进度从背景色淡入到 overlay 灰,与整行变灰
    // ([`dim_unfocused`])同一进度源,文字与底色同步浮现/消隐。
    let fade = state.focus_fade.eased_in_out();
    if fade > 0 {
        spans.push(Span::styled(
            "◌ not focused  ",
            Style::new().fg(lerp_color(
                theme.base,
                theme.overlay,
                u64::from(fade),
                /*denom*/ 1000,
            )),
        ));
    }
    // 无音频设备降级:常驻徽标提示「能浏览/编队列,但没声」。放右段最前(右对齐块的左侧),
    // 真实终端任意宽度都可见(左段宽度随窗口缩放,放不下)。
    if state.playback.audio_backend == AudioBackend::Null {
        spans.push(Span::styled(
            "⚠ 无音频设备  ",
            Style::new().fg(theme.peach).add_modifier(Modifier::BOLD),
        ));
    }
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

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::render::theme::Theme;

    /// Playlists tab 态。
    #[test]
    fn top_status_playlists_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 1))?;
        let state = crate::test_support::state_with_playlists()?;
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        // 版本号(`mineral vX.Y.Z`)随每次 version bump 变,过滤成占位符避免快照失效。
        insta::with_settings!({
            filters => vec![(r"v\d+\.\d+\.\d+", "v[VERSION]")],
            prepend_module_to_snapshot => false,
            description => "顶栏:Playlists 标签态(版本号已过滤)"
        }, {
            insta::assert_snapshot!(t.backend());
        });
        Ok(())
    }

    /// Library tab + queue 打开。
    #[test]
    fn top_status_library_queue_open_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 1))?;
        let state = crate::test_support::state_with_tracks()?;
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        // 版本号(`mineral vX.Y.Z`)随每次 version bump 变,过滤成占位符避免快照失效。
        insta::with_settings!({
            filters => vec![(r"v\d+\.\d+\.\d+", "v[VERSION]")],
            prepend_module_to_snapshot => false,
            description => "顶栏:Library 标签 + 队列打开(版本号已过滤)"
        }, {
            insta::assert_snapshot!(t.backend());
        });
        Ok(())
    }

    /// 渲染一帧顶栏,取左上角(`▌` 标记,聚焦态为 accent 色)的前景色。
    fn origin_fg(
        state: &crate::runtime::state::AppState,
        theme: &Theme,
    ) -> color_eyre::Result<ratatui::style::Color> {
        use color_eyre::eyre::eyre;
        let mut t = Terminal::new(TestBackend::new(80, 1))?;
        t.draw(|f| super::draw(f, f.area(), state, theme))?;
        Ok(t.backend()
            .buffer()
            .cell((0, 0))
            .ok_or_else(|| eyre!("cell (0,0) 应存在"))?
            .fg)
    }

    /// 终端失焦:整行前景向背景渐变——中途帧介于聚焦色与终态色之间,三态互异。
    #[test]
    fn top_status_unfocused_fade_dims_foreground() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut state = crate::test_support::state_with_playlists()?;
        let focused = origin_fg(&state, &theme)?;
        state.focused = false;
        state.focus_fade.enter();
        // 默认 288ms / 16ms tick = 18 拍;9 拍是中途帧。
        for _ in 0..9 {
            state.focus_fade.tick();
        }
        let mid = origin_fg(&state, &theme)?;
        for _ in 0..30 {
            state.focus_fade.tick();
        }
        let settled = origin_fg(&state, &theme)?;
        assert_ne!(mid, focused, "中途帧应已偏离聚焦色");
        assert_ne!(mid, settled, "中途帧应未到终态色");
        assert_ne!(settled, focused, "终态应比聚焦态更暗");
        Ok(())
    }

    /// 终端失焦渐变推满:顶栏右段出现 `◌ not focused` 徽标。
    #[test]
    fn top_status_unfocused_badge_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 1))?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.focused = false;
        state.focus_fade.enter();
        for _ in 0..30 {
            state.focus_fade.tick();
        }
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        // 版本号(`mineral vX.Y.Z`)随每次 version bump 变,过滤成占位符避免快照失效。
        insta::with_settings!({
            filters => vec![(r"v\d+\.\d+\.\d+", "v[VERSION]")],
            prepend_module_to_snapshot => false,
            description => "顶栏:终端失焦徽标(版本号已过滤)"
        }, {
            insta::assert_snapshot!(t.backend());
        });
        Ok(())
    }

    /// 无音频设备降级:顶栏常驻 `⚠ 无音频设备` 徽标。
    #[test]
    fn top_status_audio_null_badge_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 1))?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.playback.audio_backend = mineral_audio::AudioBackend::Null;
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        // 版本号(`mineral vX.Y.Z`)随每次 version bump 变,过滤成占位符避免快照失效。
        insta::with_settings!({
            filters => vec![(r"v\d+\.\d+\.\d+", "v[VERSION]")],
            prepend_module_to_snapshot => false,
            description => "顶栏:无音频设备降级徽标(版本号已过滤)"
        }, {
            insta::assert_snapshot!(t.backend());
        });
        Ok(())
    }
}
