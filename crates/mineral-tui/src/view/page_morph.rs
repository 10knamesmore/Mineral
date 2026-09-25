//! 页面形变的内容合成：端点排版固定，持续内容在当前几何上独立绘制。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::frame::{draw_fullscreen_cover, nonempty};

use crate::app::App;
use crate::components::layout::browse::{lyrics, now_playing, sidebar, spectrum};
use crate::components::layout::flight;
use crate::components::layout::search::{detail, panel};
use crate::components::layout::shared::compute::Areas;
use crate::components::layout::shared::marquee::MarqueeCtx;
use crate::components::layout::shared::waveform::WaveformCtx;
use crate::components::layout::shared::{text, top_status, transform, transport};
use crate::components::layout::transition;
use crate::runtime::state::SearchFocus;

/// 浏览与搜索共用一次进度；两列内容在途中交接，封面与播放信息持续移动。
pub(super) fn search(frame: &mut Frame<'_>, normal: &Areas, search: &Areas, app: &App) {
    let active = &app.state.channel_search.active;
    let raw = active.raw();
    let eased = active.eased_in_out();
    let areas = transform::morph_search(normal, search, eased);
    let cover = flight::plan(normal, search, &app.state);
    search_columns(frame, normal, search, &areas, cover.is_some(), app);
    disappearing_status(frame, normal.top_status, areas.top_status, raw, app);
    if let (Some(endpoint), Some(current)) = (search.search_prompt, areas.search_prompt) {
        let target = transition::capture(frame, endpoint, |frame| {
            panel::draw_prompt(
                frame,
                endpoint,
                &app.state.channel_search,
                &app.theme,
                app.state.cfg.sources(),
                app.state.channel_search.focus == SearchFocus::Prompt,
            );
        });
        transition::panel(frame, None, Some(&target), current, raw, &app.theme);
    }
    if let (Some(endpoint), Some(current)) = (normal.lyrics, areas.lyrics) {
        let source = transition::capture(frame, endpoint, |frame| {
            lyrics::draw(
                frame,
                endpoint,
                &app.state,
                &app.theme,
                lyrics::LyricMode::Compact,
            );
        });
        transition::panel(frame, Some(&source), None, current, raw, &app.theme);
    }
    if let (Some(endpoint), Some(current)) = (normal.spectrum, areas.spectrum) {
        let source = transition::capture(frame, endpoint, |frame| {
            spectrum::draw(frame, endpoint, &app.state.spectrum, &app.theme);
        });
        transition::panel(frame, Some(&source), None, current, raw, &app.theme);
    }
    if let Some(cover) = cover {
        flight::render(frame, &cover, eased, &app.state, &app.theme);
    }
    persistent_transport(frame, areas.transport, app);
    if let Some(prompt) = areas.search_prompt {
        panel::draw_prompt_dropdown(frame, prompt, &app.state, &app.theme);
    }
}

/// 搜索两列各自捕获两端布局；渲染内容不再取决于动画方向。
fn search_columns(
    frame: &mut Frame<'_>,
    normal: &Areas,
    search: &Areas,
    current: &Areas,
    cover_in_flight: bool,
    app: &App,
) {
    let state = &app.state;
    let theme = &app.theme;
    let raw = state.channel_search.active.raw();
    let from = transition::capture(frame, normal.left, |frame| {
        sidebar::draw(frame, normal.left, state, theme);
    });
    let to = transition::capture(frame, search.left, |frame| {
        panel::draw_results(
            frame,
            search.left,
            state,
            theme,
            state.channel_search.focus == SearchFocus::Results,
        );
    });
    transition::panel(frame, Some(&from), Some(&to), current.left, raw, theme);
    let from = normal.right.map(|area| {
        transition::capture(frame, area, |frame| {
            now_playing::draw(frame, area, state, theme, cover_in_flight);
        })
    });
    let to = search.right.map(|area| {
        transition::capture(frame, area, |frame| {
            detail::draw(
                frame,
                area,
                state,
                theme,
                state.channel_search.focus == SearchFocus::Detail,
                cover_in_flight,
            );
        })
    });
    if let Some(area) = current.right {
        transition::panel(frame, from.as_ref(), to.as_ref(), area, raw, theme);
    }
}

/// 浏览与全屏：列表固定排版退场，歌词按两端行距交接，当前歌词独立移动。
pub(super) fn fullscreen(frame: &mut Frame<'_>, normal: &Areas, full: &Areas, app: &App) {
    let active = &app.state.browse.fullscreen;
    let raw = active.raw();
    let eased = active.eased_in_out();
    let areas = transform::morph_areas(normal, full, eased);
    let cover = flight::plan_fullscreen(normal, full, &app.state);
    disappearing_browse(frame, normal, &areas, cover.is_some(), app);
    if let Some(area) = areas.spectrum.and_then(nonempty) {
        spectrum::draw(frame, area, &app.state.spectrum, &app.theme);
    }
    let lyrics = full
        .lyrics
        .map(|to| lyrics::LyricTransition::new(normal.lyrics, to, &app.state, raw));
    if let Some(lyrics) = &lyrics {
        lyrics.draw_panel(frame, &app.theme);
    }
    match cover {
        Some(cover) => flight::render(frame, &cover, eased, &app.state, &app.theme),
        None => {
            if let Some(area) = areas.cover.and_then(nonempty) {
                draw_fullscreen_cover(frame, area, full.cover, app);
            }
        }
    }
    persistent_transport(frame, areas.transport, app);
    if let Some(lyrics) = &lyrics {
        lyrics.draw_current(frame, &app.theme);
    }
}

/// 全屏退场面板只改变可见窗口，不让临时宽高重新排版正文。
fn disappearing_browse(
    frame: &mut Frame<'_>,
    normal: &Areas,
    current: &Areas,
    cover_in_flight: bool,
    app: &App,
) {
    let raw = app.state.browse.fullscreen.raw();
    disappearing_status(frame, normal.top_status, current.top_status, raw, app);
    let source = transition::capture(frame, normal.left, |frame| {
        sidebar::draw(frame, normal.left, &app.state, &app.theme);
    });
    transition::panel(frame, Some(&source), None, current.left, raw, &app.theme);
    if let (Some(endpoint), Some(area)) = (normal.right, current.right) {
        let source = transition::capture(frame, endpoint, |frame| {
            now_playing::draw(frame, endpoint, &app.state, &app.theme, cover_in_flight);
        });
        transition::panel(frame, Some(&source), None, area, raw, &app.theme);
    }
}

/// 顶栏采用浏览端点排版，随其窗口和文字透明度退场。
fn disappearing_status(frame: &mut Frame<'_>, from: Rect, to: Rect, raw: u16, app: &App) {
    let source = transition::capture(frame, from, |frame| {
        top_status::draw(frame, from, &app.state, &app.theme);
    });
    transition::content(frame, Some(&source), None, to, raw, &app.theme);
}

/// 播放信息只绘制一次，不吃页面透明度，也不在两个宽度间反复查询同一个 marquee 槽。
fn persistent_transport(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let theme = &app.theme;
    transition::clear_symbols(frame.buffer_mut(), area);
    let fade_to = match text::center_bg(frame, area) {
        background @ Color::Rgb(..) => background,
        _ => theme.base,
    };
    transport::draw(
        frame,
        area,
        &app.state.playback,
        &app.state.transport,
        &MarqueeCtx::new(&app.state, theme, fade_to),
        &WaveformCtx::new(&app.state, theme),
        theme,
    );
}
