//! 整屏形变(Browse ↔ Search、Browse ↔ Fullscreen)的封面飞行层:形变期把主封面从面板
//! 内容里抽出来单独画,rect 在两端封面位间插值、两图按进度 fade 像素合成,halfblock 直出
//! (纯 cell 无带外 image-id，逐帧重画安全)；同时按两端稳态尺寸准备终端图片，落定后
//! 可以直接 place。根治端点整帧瞬换，以及两端各自收放造成的不连续。
//!
//! 开层条件随形变而异:search 端点内容整帧瞬换、无自己的图收放路径,故 [`plan`] 单端就绪
//! 即开(退化为该端收缩 / 生长)；fullscreen 侧本就有独立收放兜底(halfblock / preview / 空白 /
//! 待机唱片纹),单端飞行不优于现状还会顶掉兜底,故 [`plan_fullscreen`] 两端都就绪才开。
//! 不开层时面板保持自画,调用方据计划是否 `Some` 决定抑制面板主图防双画。

use mineral_model::MediaUrl;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::components::layout::browse::now_playing::main_cover;
use crate::components::layout::search::detail;
use crate::components::layout::shared::compute::Areas;
use crate::components::layout::shared::transform::{lerp_rect, zero_center};
use crate::image::{BlendStyle, ImageContent, ImageRenderPhase};
use crate::runtime::state::{AppState, EntityRef};

/// 飞行一端：端点稳态的封面区几何与图片身份。
struct FlightEnd {
    /// 端点稳态封面区。准备与渲染使用同一 cell 区域，正方化由图片引擎负责。
    area: Rect,

    /// 该端封面 URL(预热编码用)。
    url: MediaUrl,
}

/// 一次 page morph 的封面飞行计划:两端至少一端就绪。
pub(crate) struct FlightPlan {
    /// browse 端(进度 0 端):now_playing 主封面。
    from: Option<FlightEnd>,

    /// search 端(进度 1000 端):detail 栈顶帧头图。
    to: Option<FlightEnd>,
}

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

/// browse ↔ fullscreen 形变的封面飞行计划:browse 主封面 ↔ 全屏封面(在播曲)。
/// **两端都就绪才开**(理由见模块文档)。
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

/// fullscreen 端:全屏封面区 + 在播曲封面。
fn fullscreen_end(full: &Areas, state: &AppState) -> Option<FlightEnd> {
    let area = full.cover?;
    let track = state.playback.track.as_ref()?;
    let url = track.cover_url.clone()?;
    resolve_end(area, url, state)
}

/// browse 端:now_playing 面板内主封面区 + 当前选中实体封面(几何与面板绘制共享同一源)。
fn browse_end(normal: &Areas, state: &AppState) -> Option<FlightEnd> {
    let panel = normal.right?;
    let [cover_area, _, _] = main_cover::sections(panel)?;
    let url = main_cover::url(state)?;
    resolve_end(cover_area, url, state)
}

/// search 端:detail 面板头图区 + 栈顶帧实体封面(几何与面板绘制共享同一源)。
fn detail_end(search: &Areas, state: &AppState) -> Option<FlightEnd> {
    let panel = search.right?;
    let dframe = state.channel_search.active_results()?.detail.current()?;
    let is_artist = matches!(dframe.entity, EntityRef::Artist(_));
    let cover_area = detail::header_cover_area(panel, is_artist)?;
    resolve_end(cover_area, dframe.entity.cover().cloned()?, state)
}

/// 端就绪判定：有 URL 且图片已解码。
fn resolve_end(area: Rect, url: MediaUrl, state: &AppState) -> Option<FlightEnd> {
    state
        .images
        .cache
        .contains_key(&url)
        .then_some(FlightEnd { area, url })
}

/// 画一帧飞行层(叠在面板之上):双端 fade 合成、单端独图收放,halfblock 直出;
/// 并按两端稳态尺寸预热编码(按 `(url, dims)` 去重,无逐帧 churn)。
///
/// # Params:
///   - `plan`: [`plan`] 产出的飞行计划
///   - `progress`: morph 进度(已缓动千分比,0 = browse 端、1000 = search 端)
///   - `state`: 图片缓存与终端成品状态
pub(crate) fn render(frame: &mut Frame<'_>, plan: &FlightPlan, progress: u16, state: &AppState) {
    let square = |area: Rect| state.images.square_area(area);
    match (&plan.from, &plan.to) {
        (Some(from), Some(to)) => {
            let rect = lerp_rect(square(from.area), square(to.area), progress);
            state.images.render(
                ImageContent::Blend {
                    from: &from.url,
                    to: &to.url,
                    progress,
                    style: BlendStyle::Fade,
                },
                rect,
                frame.buffer_mut(),
                ImageRenderPhase::Resizing,
            );
        }
        // 单端:图沿自身中心收缩 / 生长(与消失面板的 collapse 语义同款),不强行合成。
        (Some(from), None) => {
            let sq = square(from.area);
            let rect = lerp_rect(sq, zero_center(sq), progress);
            state.images.render(
                ImageContent::Display {
                    url: Some(&from.url),
                },
                rect,
                frame.buffer_mut(),
                ImageRenderPhase::Resizing,
            );
        }
        (None, Some(to)) => {
            let sq = square(to.area);
            let rect = lerp_rect(zero_center(sq), sq, progress);
            state.images.render(
                ImageContent::Display { url: Some(&to.url) },
                rect,
                frame.buffer_mut(),
                ImageRenderPhase::Resizing,
            );
        }
        (None, None) => {}
    }
    // 两端稳态协议都预热:落定(任一方向)kitty 直接 place 零闪。`(url, dims)` 去重,
    // 每帧调用无 churn。
    for end in [plan.from.as_ref(), plan.to.as_ref()].into_iter().flatten() {
        state.images.prepare(&end.url, end.area);
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use ratatui::layout::Rect;

    use crate::components::layout::shared::compute::{compute, compute_search};
    use crate::test_support::app_in_search_morph;

    /// 两端完整解码图都未入缓存时不开飞行层。
    #[test]
    fn plan_none_without_cached_images() -> color_eyre::Result<()> {
        let app = app_in_search_morph(/*cache_browse*/ false, /*cache_detail*/ false)?;
        let cfg = app.state.cfg.tui().layout().clone();
        let area = Rect::new(0, 0, 120, 40);
        let normal = compute(area, &cfg);
        let search = compute_search(area, &cfg);
        assert!(
            super::plan(&normal, &search, &app.state).is_none(),
            "无缓存图不应开飞行层"
        );
        Ok(())
    }

    /// 仅 browse 端图入缓存:单端计划(from 就绪、to 缺席),渲染侧据此走单图收缩。
    #[test]
    fn plan_from_only_when_browse_cached() -> color_eyre::Result<()> {
        let app = app_in_search_morph(/*cache_browse*/ true, /*cache_detail*/ false)?;
        let cfg = app.state.cfg.tui().layout().clone();
        let area = Rect::new(0, 0, 120, 40);
        let normal = compute(area, &cfg);
        let search = compute_search(area, &cfg);
        let plan = super::plan(&normal, &search, &app.state)
            .ok_or_else(|| eyre!("browse 端图已缓存,应有单端计划"))?;
        assert!(plan.from.is_some(), "browse 端应就绪");
        assert!(plan.to.is_none(), "detail 端图未缓存应缺席");
        Ok(())
    }
}
