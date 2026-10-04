//! 页面封面选择与飞行组件的接线。

use crate::components::layout::browse::now_playing::main_cover;
use crate::components::layout::flight::{FlightContent, FlightEnd, FlightPlan};
use crate::components::layout::search::detail;
use crate::components::layout::shared::compute::Areas;
use crate::runtime::state::{AppState, EntityRef};
use mineral_model::MediaUrl;
use ratatui::layout::Rect;

/// 按两端点布局与当前状态解析封面飞行计划。两端图都缺(无 url / 未入缓存 / 面板画不下)
/// 返回 `None`——调用方保持面板自画,不抑制。
///
/// # Params:
///   - `normal`: browse 端点布局(`compute` 产出)
///   - `search`: search 端点布局(`compute_search` 产出)
///
/// # Return:
///   至少一端就绪的飞行计划;两端全缺为 `None`。
pub(crate) fn plan(normal: &Areas, search: &Areas, state: &AppState) -> Option<FlightPlan> {
    let from = browse_end(normal, state);
    let to = detail_end(search, state);
    (from.is_some() || to.is_some()).then_some(FlightPlan { from, to })
}

/// 浏览封面 ↔ 全屏在播封面或待机唱片；两端内容都可绘制时开启飞行。
///
/// # Params:
///   - `normal`: browse 端点布局(`compute` 产出)
///   - `full`: 全屏端点布局(`compute_fullscreen` 产出)
///
/// # Return:
///   两端都就绪的飞行计划;任一端缺席为 `None`。
pub(crate) fn plan_fullscreen(
    normal: &Areas,
    full: &Areas,
    state: &AppState,
) -> Option<FlightPlan> {
    let from = browse_end(normal, state)?;
    let to = fullscreen_end(full, state)?;
    Some(FlightPlan {
        from: Some(from),
        to: Some(to),
    })
}

/// 全屏端：无在播曲时采用待机唱片，有在播曲时等待其封面解码。
fn fullscreen_end(full: &Areas, state: &AppState) -> Option<FlightEnd> {
    let area = full.cover?;
    let Some(track) = state.models.playback.track.as_ref() else {
        return Some(FlightEnd {
            area,
            content: FlightContent::Vinyl,
        });
    };
    let url = track.cover_url.clone()?;
    resolve_end(area, url, state)
}

/// browse 端:now_playing 面板内主封面区 + 当前选中实体封面(几何与面板绘制共享同一源)。
fn browse_end(normal: &Areas, state: &AppState) -> Option<FlightEnd> {
    let panel = normal.right?;
    let [cover_area, _, _] = main_cover::sections(panel)?;
    let url = crate::view::now_playing::url(state)?;
    resolve_end(cover_area, url, state)
}

/// search 端:detail 面板头图区 + 栈顶帧实体封面(几何与面板绘制共享同一源)。
fn detail_end(search: &Areas, state: &AppState) -> Option<FlightEnd> {
    let panel = search.right?;
    let dframe = state.ui.channel_search.active_results()?.detail.current()?;
    let is_artist = matches!(dframe.entity, EntityRef::Artist(_));
    let cover_area = detail::header_cover_area(panel, is_artist)?;
    resolve_end(cover_area, dframe.entity.cover().cloned()?, state)
}

/// 端就绪判定：有 URL 且图片已解码。
fn resolve_end(area: Rect, url: MediaUrl, state: &AppState) -> Option<FlightEnd> {
    state
        .resources
        .images
        .ready()
        .contains_decoded(&url)
        .then_some(FlightEnd {
            area,
            content: FlightContent::Cover(url),
        })
}
