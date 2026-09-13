//! 共享歌词的可见性、字级样式、端点一致性和无共享端点时的退化行为。

use color_eyre::eyre::eyre;
use mineral_model::{LineKind, LyricLine, Lyrics, Word};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};

use super::{LyricMode, LyricTransition, draw};
use crate::components::layout::shared::transform::{lerp_rect, zero_center};
use crate::components::layout::transition;
use crate::render::anim::{ease_in_out, lerp_u16};
use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, LyricExtra};

/// 不同尺寸、不同位置的两个端点，当前行分别位于 y=25 和 y=16。
const FROM: Rect = Rect::new(0, 20, 32, 10);
/// 全屏端点，足以容纳手动滚动前后的邻行。
const TO: Rect = Rect::new(40, 0, 48, 32);
/// 验证文字对实际背景取色，避免把 Reset 当作已铺氛围场。
const BACKGROUND: Color = Color::Rgb(12, 24, 36);
/// 当前行文本仅出现一次，便于检查端点重复绘制。
const CURRENT: &str = "shared lyric";

/// 串联真实生产入口，供不需要插入其他内容的歌词转场测试复用。
fn draw_morph(
    frame: &mut Frame<'_>,
    from: Option<Rect>,
    to: Rect,
    state: &AppState,
    theme: &Theme,
    raw_progress: u16,
) {
    let transition = LyricTransition::new(from, to, state, raw_progress);
    transition.draw_panel(frame, theme);
    transition.draw_current(frame, theme);
}

/// 可手动滚离当前行的同步歌词，当前原文下有一条翻译。
fn lyric_state() -> color_eyre::Result<AppState> {
    let mut state = crate::test_support::state_with_lyrics(LyricExtra::Translation, false)?;
    let id = state
        .playback
        .track
        .as_ref()
        .ok_or_else(|| eyre!("缺在播曲"))?
        .id
        .clone();
    let mut current = LyricLine::timed(2_000, CURRENT);
    current.translation = Some("current translation".to_owned());
    let mut lines = vec![LyricLine::timed(1_000, "previous lyric"), current];
    lines.extend((0..30u64).map(|i| LyricLine::timed(10_000 + i * 1_000, format!("next {i}"))));
    state.library.lyrics.insert(id, Lyrics { lines });
    state.playback.position_ms = 2_500;
    Ok(state)
}

/// 在同一背景上绘制，返回可比较字符、颜色与修饰的完整缓冲。
fn render(paint: impl FnOnce(&mut Frame<'_>)) -> color_eyre::Result<Buffer> {
    let mut terminal = Terminal::new(TestBackend::new(96, 40))?;
    terminal.draw(|frame| {
        frame.render_widget(
            Block::new().style(Style::new().bg(BACKGROUND)),
            frame.area(),
        );
        paint(frame);
    })?;
    Ok(terminal.backend().buffer().clone())
}

/// 查找测试夹具中的 ASCII 原文，不用于产品绘制或身份判断。
fn text_positions(buffer: &Buffer, text: &str) -> Vec<(u16, u16)> {
    let mut positions = Vec::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            let matches = text.chars().enumerate().all(|(offset, ch)| {
                u16::try_from(offset)
                    .ok()
                    .and_then(|offset| buffer.cell((x.saturating_add(offset), y)))
                    .is_some_and(|cell| cell.symbol().chars().eq(std::iter::once(ch)))
            });
            if matches {
                positions.push((x, y));
            }
        }
    }
    positions
}

/// 当前歌词必须完整且只出现一次，避免原端点文字与共享行叠印。
fn current_position(buffer: &Buffer, text: &str) -> color_eyre::Result<(u16, u16)> {
    let positions = text_positions(buffer, text);
    assert_eq!(positions.len(), 1, "当前原文应只显示一次: {positions:?}");
    positions
        .first()
        .copied()
        .ok_or_else(|| eyre!("缺当前原文"))
}

/// 页面中途文字变暗时，已落定的当前歌词仍以完整高亮沿端点位置移动。
#[test]
fn current_line_stays_bright_and_moves_once() -> color_eyre::Result<()> {
    let theme = crate::test_support::default_theme()?;
    let state = lyric_state()?;
    for raw in [250, 500, 750] {
        let buffer = render(|frame| draw_morph(frame, Some(FROM), TO, &state, &theme, raw))?;
        let (x, y) = current_position(&buffer, CURRENT)?;
        assert_eq!(
            y,
            lerp_u16(25, 16, ease_in_out(raw)),
            "当前行沿实际端点移动"
        );
        for offset in 0..u16::try_from(CURRENT.len())? {
            let cell = buffer
                .cell((x + offset, y))
                .ok_or_else(|| eyre!("缺当前行 cell"))?;
            assert_eq!(cell.fg, theme.accent, "共享行不吃页面透明度，raw={raw}");
            assert!(cell.modifier.contains(Modifier::BOLD));
        }
        if raw == 250 {
            let translations = text_positions(&buffer, "current translation");
            assert_eq!(translations.len(), 1, "抑制原文时仍保留副行占位和内容");
        }
    }
    Ok(())
}

/// 持续内容可以夹在面板与共享行之间绘制，歌词最终压在边框上且没有端点副本。
#[test]
fn current_line_overlays_persistent_content_after_panel() -> color_eyre::Result<()> {
    let theme = crate::test_support::default_theme()?;
    let state = lyric_state()?;
    let transition = LyricTransition::new(Some(FROM), TO, &state, 500);
    let buffer = render(|frame| {
        transition.draw_panel(frame, &theme);
        assert!(
            text_positions(frame.buffer_mut(), CURRENT).is_empty(),
            "面板阶段应抑制当前原文，留给最后的共享层"
        );
        // 两端当前行 y=25 与 y=16，半程落在 y=20，模拟 transport 上边框穿过它。
        frame.render_widget(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(theme.yellow))
                .title("transport"),
            Rect::new(0, 20, 96, 4),
        );
        frame
            .buffer_mut()
            .set_string(22, 20, "artist", Style::new().fg(theme.subtext));
        frame
            .buffer_mut()
            .set_string(50, 20, "album", Style::new().fg(theme.subtext));
        transition.draw_current(frame, &theme);
    })?;
    let (x, y) = current_position(&buffer, CURRENT)?;
    assert_eq!(y, 20, "当前歌词仍在共享行位置完整显示");
    for offset in 0..u16::try_from(CURRENT.len())? {
        let cell = buffer
            .cell((x + offset, y))
            .ok_or_else(|| eyre!("缺当前行 cell"))?;
        assert_eq!(cell.fg, theme.accent, "歌词应覆盖持续内容的边框");
    }
    assert_eq!(
        text_positions(&buffer, "transport").len(),
        1,
        "共享行外的持续内容保持可见"
    );
    assert_eq!(text_positions(&buffer, "artist"), [(22, 20)]);
    assert_eq!(text_positions(&buffer, "album"), [(50, 20)]);
    Ok(())
}

/// 共享行中的已唱、未唱字保留原 wipe 颜色，不能全部改成 accent 或叠加页面淡化。
#[test]
fn word_wipe_and_unlit_characters_survive_morph() -> color_eyre::Result<()> {
    let theme = crate::test_support::default_theme()?;
    let mut state = lyric_state()?;
    let id = state
        .playback
        .track
        .as_ref()
        .ok_or_else(|| eyre!("缺在播曲"))?
        .id
        .clone();
    let current = state
        .library
        .lyrics
        .get_mut(&id)
        .and_then(|lyrics| lyrics.lines.get_mut(1))
        .ok_or_else(|| eyre!("缺当前歌词"))?;
    current.kind = LineKind::Words {
        dur_ms: 3_000,
        words: vec![
            Word {
                start_ms: 2_000,
                dur_ms: 1_000,
                text: "A".to_owned(),
            },
            Word {
                start_ms: 3_000,
                dur_ms: 2_000,
                text: "B".to_owned(),
            },
        ],
    };
    let unlit = theme
        .text_over(BACKGROUND, theme.text_alpha.strong)
        .ok_or_else(|| eyre!("真彩背景应产生未唱字颜色"))?;
    let expected = [lerp_color(unlit, theme.accent, 500, 1000), unlit];
    for raw in [250, 500, 750] {
        let buffer = render(|frame| draw_morph(frame, Some(FROM), TO, &state, &theme, raw))?;
        let (x, y) = current_position(&buffer, "AB")?;
        for (offset, expected) in expected.iter().enumerate() {
            let cell = buffer
                .cell((x + u16::try_from(offset)?, y))
                .ok_or_else(|| eyre!("缺逐字 cell"))?;
            assert_eq!(cell.fg, *expected, "逐字颜色只由播放进度决定");
            assert!(cell.modifier.contains(Modifier::BOLD));
        }
    }
    Ok(())
}

/// 刚换行时保留 Immersive 自身的行级淡入，不把共享行强行提成满 accent。
#[test]
fn line_highlight_keeps_its_own_transition() -> color_eyre::Result<()> {
    let theme = crate::test_support::default_theme()?;
    let mut state = lyric_state()?;
    state.playback.position_ms = 2_000;
    let from = render(|frame| draw(frame, FROM, &state, &theme, LyricMode::Compact))?;
    let to = render(|frame| draw(frame, TO, &state, &theme, LyricMode::Immersive))?;
    let from_position = current_position(&from, CURRENT)?;
    let to_position = current_position(&to, CURRENT)?;
    let from_color = from
        .cell(from_position)
        .ok_or_else(|| eyre!("缺常规当前行"))?
        .fg;
    let to_color = to
        .cell(to_position)
        .ok_or_else(|| eyre!("缺全屏当前行"))?
        .fg;
    assert_ne!(from_color, to_color, "夹具必须处于行级高亮交接中");
    let buffer = render(|frame| draw_morph(frame, Some(FROM), TO, &state, &theme, 500))?;
    let position = current_position(&buffer, CURRENT)?;
    let current = buffer.cell(position).ok_or_else(|| eyre!("缺共享行"))?;
    assert_eq!(position.1, lerp_u16(from_position.1, to_position.1, 500));
    assert_eq!(current.fg, lerp_color(from_color, to_color, 500, 1000));
    assert!(
        !current.modifier.contains(Modifier::BOLD),
        "半程高亮尚不加粗"
    );
    Ok(())
}

/// 两端逐 cell 对齐已有稳态，包括副歌词、手动滚动及歌词自身处于跨行中帧。
#[test]
fn endpoints_match_steady_lyrics() -> color_eyre::Result<()> {
    let theme = crate::test_support::default_theme()?;
    for (words, position_ms, manual) in [
        (false, 62_000, false),
        (false, 66_510, true),
        (true, 66_510, false),
        (true, 62_000, true),
    ] {
        let mut state = crate::test_support::state_with_lyrics(LyricExtra::Translation, words)?;
        state.playback.position_ms = position_ms;
        if manual {
            state.debug_scroll_lyrics_to_settled(2);
        }
        for (raw, area, mode) in [
            (0, FROM, LyricMode::Compact),
            (1000, TO, LyricMode::Immersive),
        ] {
            let steady = render(|frame| draw(frame, area, &state, &theme, mode))?;
            let morph = render(|frame| draw_morph(frame, Some(FROM), TO, &state, &theme, raw))?;
            assert_eq!(
                morph, steady,
                "端点 raw={raw} 应等于稳态，words={words}, manual={manual}"
            );
        }
    }
    Ok(())
}

/// 不抽取共享行的普通面板转场，作为无真实双端当前行时的行为基准。
fn ordinary_morph(
    state: &AppState,
    theme: &Theme,
    from: Option<Rect>,
    raw: u16,
) -> color_eyre::Result<Buffer> {
    render(|frame| {
        let from_buffer = from.map(|area| {
            transition::capture(frame, area, |frame| {
                draw(frame, area, state, theme, LyricMode::Compact);
            })
        });
        let to_buffer = transition::capture(frame, TO, |frame| {
            draw(frame, TO, state, theme, LyricMode::Immersive);
        });
        let area = lerp_rect(
            from.unwrap_or_else(|| zero_center(TO)),
            TO,
            ease_in_out(raw),
        );
        transition::panel(
            frame,
            from_buffer.as_ref(),
            Some(&to_buffer),
            area,
            raw,
            theme,
        );
    })
}

/// Broken、手动滚出、前奏、无歌词或缺常规面板都只走普通淡化，不伪造中心高亮。
#[test]
fn unavailable_current_line_uses_ordinary_fade() -> color_eyre::Result<()> {
    let theme = crate::test_support::default_theme()?;
    let mut broken = lyric_state()?;
    let song = broken
        .playback
        .track
        .as_ref()
        .ok_or_else(|| eyre!("缺在播曲"))?;
    let duration = song.duration_ms.ok_or_else(|| eyre!("夹具缺时长"))?;
    broken.playback.media_info = Some(mineral_model::PlaybackMediaInfo {
        song_id: song.id.clone(),
        bitrate_bps: None,
        quality: mineral_model::BitRate::Standard,
        size: None,
        format: Some(mineral_model::AudioFormat::Aac),
        bit_depth: None,
        substituted: true,
    });
    broken.playback.engine_duration_ms = Some(duration + 30_000);
    assert_eq!(
        broken.playback.sync_trust(),
        crate::runtime::playback::SyncTrust::Broken
    );
    let mut scrolled = lyric_state()?;
    scrolled.debug_scroll_lyrics_to_settled(20);
    let mut prelude = lyric_state()?;
    prelude.playback.position_ms = 0;
    for (label, state, from) in [
        ("Broken", broken, Some(FROM)),
        ("手动滚出", scrolled, Some(FROM)),
        ("前奏", prelude, Some(FROM)),
        ("无歌词", AppState::test_default()?, Some(FROM)),
        ("缺常规面板", lyric_state()?, None),
    ] {
        for raw in [250, 500, 750] {
            let expected = ordinary_morph(&state, &theme, from, raw)?;
            let actual = render(|frame| draw_morph(frame, from, TO, &state, &theme, raw))?;
            assert_eq!(actual, expected, "{label} 不应抽出共享行，raw={raw}");
        }
    }
    Ok(())
}
