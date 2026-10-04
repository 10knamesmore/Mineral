//! 页面形变的内容合成：端点排版固定，持续内容在当前几何上独立绘制。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::frame::{draw_fullscreen_cover, nonempty};

use super::preparation::FrameView;
use crate::components::layout::browse::{lyrics, spectrum};
use crate::components::layout::flight;
use crate::components::layout::search::panel;
use crate::components::layout::shared::compute::Areas;
use crate::components::layout::shared::{text, transform};
use crate::components::layout::transition;
use crate::runtime::state::SearchFocus;

/// 浏览与搜索共用一次进度；两列内容在途中交接，封面与播放信息持续移动。
pub(super) fn search(frame: &mut Frame<'_>, normal: &Areas, search: &Areas, app: &FrameView<'_>) {
    let active = &app.search.page.active;
    let raw = active.raw();
    let eased = active.eased_in_out();
    let areas = transform::morph_search(normal, search, eased);
    let cover = app.search_flight.as_ref();
    search_columns(frame, normal, search, &areas, cover.is_some(), app);
    disappearing_status(frame, normal.top_status, areas.top_status, raw, app);
    if let (Some(endpoint), Some(current)) = (search.search_prompt, areas.search_prompt) {
        let target = transition::capture(frame, endpoint, |frame| {
            panel::draw_prompt(
                frame,
                endpoint,
                app.search.page,
                app.env.theme,
                app.env.config.sources(),
                app.search.page.focus == SearchFocus::Prompt,
            );
        });
        transition::panel(frame, None, Some(&target), current, raw, app.env.theme);
    }
    if let (Some(endpoint), Some(current)) = (normal.lyrics, areas.lyrics) {
        let source = transition::capture(frame, endpoint, |frame| {
            lyrics::draw(
                frame,
                endpoint,
                &app.lyrics,
                app.env.theme,
                lyrics::LyricMode::Compact,
            );
        });
        transition::panel(frame, Some(&source), None, current, raw, app.env.theme);
    }
    if let (Some(endpoint), Some(current)) = (normal.spectrum, areas.spectrum) {
        let source = transition::capture(frame, endpoint, |frame| {
            spectrum::draw(frame, endpoint, app.spectrum, app.env.theme);
        });
        transition::panel(frame, Some(&source), None, current, raw, app.env.theme);
    }
    if let Some(cover) = cover {
        flight::render(frame, cover, eased, app.images, app.vinyl, app.env.theme);
    }
    persistent_transport(frame, areas.transport, app);
    if let Some(prompt) = areas.search_prompt {
        panel::draw_prompt_dropdown(frame, prompt, &app.search, app.env.theme);
    }
}

/// 搜索两列各自捕获两端布局；渲染内容不再取决于动画方向。
fn search_columns(
    frame: &mut Frame<'_>,
    normal: &Areas,
    search: &Areas,
    current: &Areas,
    cover_in_flight: bool,
    app: &FrameView<'_>,
) {
    let theme = app.env.theme;
    let raw = app.search.page.active.raw();
    let from = transition::capture(frame, normal.left, |frame| {
        app.browse.paint(frame, normal.left);
    });
    let to = transition::capture(frame, search.left, |frame| {
        panel::draw_results(
            frame,
            search.left,
            &app.search,
            theme,
            app.search.page.focus == SearchFocus::Results,
        );
    });
    transition::panel(frame, Some(&from), Some(&to), current.left, raw, theme);
    let from = normal.right.map(|area| {
        transition::capture(frame, area, |frame| {
            crate::components::layout::browse::now_playing::draw(
                frame,
                area,
                &app.selected,
                theme,
                cover_in_flight,
            );
        })
    });
    let to = search.right.map(|area| {
        transition::capture(frame, area, |frame| {
            crate::components::layout::search::detail::draw(
                frame,
                area,
                &app.detail,
                theme,
                app.search.page.focus == SearchFocus::Detail,
                cover_in_flight,
            );
        })
    });
    if let Some(area) = current.right {
        transition::panel(frame, from.as_ref(), to.as_ref(), area, raw, theme);
    }
}

/// 浏览与全屏：列表固定排版退场，完整歌词面板按两端行距淡化交接。
pub(super) fn fullscreen(frame: &mut Frame<'_>, normal: &Areas, full: &Areas, app: &FrameView<'_>) {
    let active = app.fullscreen;
    let raw = active.raw();
    let eased = active.eased_in_out();
    let areas = transform::morph_areas(normal, full, eased);
    let cover = app.fullscreen_flight.as_ref();
    disappearing_browse(frame, normal, &areas, cover.is_some(), app);
    if let Some(area) = areas.spectrum.and_then(nonempty) {
        spectrum::draw(frame, area, app.spectrum, app.env.theme);
    }
    if let Some(to) = full.lyrics {
        lyrics::draw_transition(frame, normal.lyrics, to, &app.lyrics, app.env.theme, raw);
    }
    match cover {
        Some(cover) => flight::render(frame, cover, eased, app.images, app.vinyl, app.env.theme),
        None => {
            if let Some(area) = areas.cover.and_then(nonempty) {
                draw_fullscreen_cover(frame, area, app);
            }
        }
    }
    persistent_transport(frame, areas.transport, app);
}

/// 全屏退场面板只改变可见窗口，不让临时宽高重新排版正文。
fn disappearing_browse(
    frame: &mut Frame<'_>,
    normal: &Areas,
    current: &Areas,
    cover_in_flight: bool,
    app: &FrameView<'_>,
) {
    let raw = app.fullscreen.raw();
    disappearing_status(frame, normal.top_status, current.top_status, raw, app);
    let source = transition::capture(frame, normal.left, |frame| {
        app.browse.paint(frame, normal.left);
    });
    transition::panel(frame, Some(&source), None, current.left, raw, app.env.theme);
    if let (Some(endpoint), Some(area)) = (normal.right, current.right) {
        let source = transition::capture(frame, endpoint, |frame| {
            crate::components::layout::browse::now_playing::draw(
                frame,
                endpoint,
                &app.selected,
                app.env.theme,
                cover_in_flight,
            );
        });
        transition::panel(frame, Some(&source), None, area, raw, app.env.theme);
    }
}

/// 顶栏采用浏览端点排版，随其窗口和文字透明度退场。
fn disappearing_status(frame: &mut Frame<'_>, from: Rect, to: Rect, raw: u16, app: &FrameView<'_>) {
    let source = transition::capture(frame, from, |frame| {
        crate::components::layout::shared::top_status::draw(
            frame,
            from,
            &app.status,
            app.env.theme,
        );
    });
    transition::content(frame, Some(&source), None, to, raw, app.env.theme);
}

/// 播放信息只绘制一次，不吃页面透明度，也使用自己的标题滚动状态。
fn persistent_transport(frame: &mut Frame<'_>, area: Rect, app: &FrameView<'_>) {
    let theme = app.env.theme;
    transition::clear_symbols(frame.buffer_mut(), area);
    let fade_to = match text::center_bg(frame, area) {
        background @ Color::Rgb(..) => background,
        _ => theme.base,
    };
    app.transport.paint(frame, area, fade_to);
}
