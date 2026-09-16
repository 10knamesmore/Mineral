//! 页面形变的封面飞行层：在两端位置之间移动，并以 halfblock 交叉渐变。
//! 未播放时，全屏端采用实时旋转的待机唱片；真实封面按稳态尺寸预热终端图片。
//!
//! 搜索形变允许单端图片收放；全屏形变要求两端内容就绪，否则沿用面板自己的预览或
//! 唱片绘制。调用方在飞行层存在时抑制面板主图，避免重复绘制。

use mineral_model::MediaUrl;
use ratatui::Frame;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::components::layout::browse::now_playing::main_cover;
use crate::components::layout::search::detail;
use crate::components::layout::shared::compute::Areas;
use crate::components::layout::shared::transform::{lerp_rect, zero_center};
use crate::components::layout::shared::vinyl;
use crate::image::{BlendStyle, ImageContent, ImageRenderPhase};
use crate::render::blit;
use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, EntityRef};

/// 飞行端点的可绘制内容；唱片仅用于没有在播曲的全屏端。
enum FlightContent {
    /// 已解码的真实封面。
    Cover(MediaUrl),

    /// 使用当前主题和旋转相位绘制的待机唱片。
    Vinyl,
}

/// 飞行一端：端点稳态的区域与内容。
struct FlightEnd {
    /// 端点稳态区域，正方几何与对应内容的稳态绘制一致。
    area: Rect,

    /// 要显示的封面或唱片。
    content: FlightContent,
}

/// 一次 page morph 的封面飞行计划:两端至少一端就绪。
pub(crate) struct FlightPlan {
    /// browse 端(进度 0 端):now_playing 主封面。
    from: Option<FlightEnd>,

    /// 进度 1000 端：搜索详情封面、全屏在播封面或待机唱片。
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
    let Some(track) = state.playback.track.as_ref() else {
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
    state.images.cache.contains_key(&url).then_some(FlightEnd {
        area,
        content: FlightContent::Cover(url),
    })
}

/// 画一帧飞行层(叠在面板之上):双端 fade 合成、单端独图收放,halfblock 直出;
/// 并按两端稳态尺寸预热编码(按 `(url, dims)` 去重,无逐帧 churn)。
///
/// # Params:
///   - `plan`: [`plan`] 产出的飞行计划
///   - `progress`: 已缓动千分比，0 为浏览端、1000 为搜索或全屏端；反向沿用同一进度
///   - `state`: 图片缓存与终端成品状态
///   - `theme`: 待机唱片本帧使用的主题
pub(crate) fn render(
    frame: &mut Frame<'_>,
    plan: &FlightPlan,
    progress: u16,
    state: &AppState,
    theme: &Theme,
) {
    let square = |end: &FlightEnd| match &end.content {
        FlightContent::Cover(_) => state.images.square_area(end.area),
        FlightContent::Vinyl => crate::image::square_cells(end.area),
    };
    match (&plan.from, &plan.to) {
        (Some(from), Some(to)) => {
            if let (FlightContent::Cover(from_url), FlightContent::Cover(to_url)) =
                (&from.content, &to.content)
            {
                let rect = lerp_rect(square(from), square(to), progress);
                state.images.render(
                    ImageContent::Blend {
                        from: from_url,
                        to: to_url,
                        progress,
                        style: BlendStyle::Fade,
                        advance: None,
                    },
                    rect,
                    frame.buffer_mut(),
                    ImageRenderPhase::Resizing,
                );
            } else {
                // 各端绘制器自行计算正方几何；这里保留原始区域，避免奇数宽度重复取整。
                let rect = lerp_rect(from.area, to.area, progress);
                let screen = frame.buffer_mut();
                let mut old = Buffer::empty(rect);
                blit::copy_window(&mut old, screen, rect, rect.x, rect.y);
                let mut new = old.clone();
                render_end(&mut old, rect, &from.content, state, theme);
                render_end(&mut new, rect, &to.content, state, theme);
                blend_halfblocks(screen, &old, &new, progress);
            }
        }
        // 单端:图沿自身中心收缩 / 生长(与消失面板的 collapse 语义同款),不强行合成。
        (Some(from), None) => {
            let sq = square(from);
            let rect = lerp_rect(sq, zero_center(sq), progress);
            render_end(frame.buffer_mut(), rect, &from.content, state, theme);
        }
        (None, Some(to)) => {
            let sq = square(to);
            let rect = lerp_rect(zero_center(sq), sq, progress);
            render_end(frame.buffer_mut(), rect, &to.content, state, theme);
        }
        (None, None) => {}
    }
    // 两端稳态协议都预热:落定(任一方向)kitty 直接 place 零闪。`(url, dims)` 去重,
    // 每帧调用无 churn。
    for end in [plan.from.as_ref(), plan.to.as_ref()].into_iter().flatten() {
        if let FlightContent::Cover(url) = &end.content {
            state.images.prepare(url, end.area);
        }
    }
}

/// 两端各自按同一移动区域绘制，封面保留原图比例，唱片沿用当前旋转相位。
fn render_end(
    buf: &mut Buffer,
    area: Rect,
    content: &FlightContent,
    state: &AppState,
    theme: &Theme,
) {
    match content {
        FlightContent::Cover(url) => state.images.render(
            ImageContent::Display { url: Some(url) },
            area,
            buf,
            ImageRenderPhase::Resizing,
        ),
        FlightContent::Vinyl => vinyl::render_to(buf, area, &state.vinyl, theme),
    }
}

/// 两端先分别叠在相同的本帧背景上，再交叉混色，避免透明留白重复叠加背景。
/// 未被任一端覆盖的 cell 保留原内容，上下半格独立取色。
fn blend_halfblocks(buf: &mut Buffer, from: &Buffer, to: &Buffer, progress: u16) {
    for (index, (old, new)) in from.content.iter().zip(&to.content).enumerate() {
        let Some(cell) = buf.cell_mut(from.pos_of(index)) else {
            continue;
        };
        if old == new || progress == 0 {
            *cell = old.clone();
        } else if progress >= 1000 {
            *cell = new.clone();
        } else {
            cell.set_char('▀')
                .set_fg(lerp_color(
                    upper_color(old),
                    upper_color(new),
                    u64::from(progress),
                    1000,
                ))
                .set_bg(lerp_color(old.bg, new.bg, u64::from(progress), 1000));
        }
    }
}

/// 留白 cell 的上半格使用背景色，不能把不可见的文字前景色混进封面。
fn upper_color(cell: &Cell) -> Color {
    if cell.symbol() == "▀" {
        cell.fg
    } else {
        cell.bg
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use color_eyre::eyre::eyre;
    use image::{DynamicImage, Rgba, RgbaImage};
    use mineral_model::MediaUrl;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use super::{FlightContent, FlightEnd, FlightPlan};
    use crate::components::layout::shared::compute::{compute, compute_search};
    use crate::components::layout::shared::vinyl;
    use crate::image::{ImageContent, ImageRenderPhase};
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

    /// 渐变两端与各自稳态逐格一致，包括长方形封面留白和当前唱片旋转相位。
    #[test]
    fn idle_vinyl_flight_matches_steady_endpoints() -> color_eyre::Result<()> {
        let mut app = app_in_search_morph(true, false)?;
        app.state.playback.track = None;
        for _ in 0..37 {
            app.state.vinyl.tick();
        }
        let screen = Rect::new(0, 0, 68, 28);
        let from_area = Rect::new(2, 4, 21, 12);
        let to_area = Rect::new(28, 3, 33, 18);
        let url = MediaUrl::remote("https://example.com/vinyl-flight.png")?;
        let plan = FlightPlan {
            from: Some(FlightEnd {
                area: from_area,
                content: FlightContent::Cover(url.clone()),
            }),
            to: Some(FlightEnd {
                area: to_area,
                content: FlightContent::Vinyl,
            }),
        };
        let mut background = Buffer::empty(screen);
        for (index, cell) in background.content.iter_mut().enumerate() {
            let shade = u8::try_from(index % 128)?;
            cell.set_fg(Color::Rgb(255, 0, 0))
                .set_bg(Color::Rgb(shade, 30, 70));
        }
        for (width, height) in [(160, 80), (80, 160), (128, 65)] {
            let pixels = RgbaImage::from_pixel(width, height, Rgba([30, 180, 230, 192]));
            app.state
                .images
                .cache
                .insert_test(&url, Arc::new(DynamicImage::ImageRgba8(pixels)));
            for progress in [0, 1000] {
                let mut expected = background.clone();
                if progress == 0 {
                    app.state.images.render(
                        ImageContent::Display { url: Some(&url) },
                        from_area,
                        &mut expected,
                        ImageRenderPhase::Resizing,
                    );
                } else {
                    vinyl::render_to(&mut expected, to_area, &app.state.vinyl, &app.theme);
                }
                let mut terminal = Terminal::new(TestBackend::new(screen.width, screen.height))?;
                terminal.draw(|frame| {
                    *frame.buffer_mut() = background.clone();
                    super::render(frame, &plan, progress, &app.state, &app.theme);
                })?;
                assert_eq!(
                    terminal.backend().buffer(),
                    &expected,
                    "{width}×{height} 封面在进度 {progress} 应与稳态内容完全相同"
                );
            }
        }
        Ok(())
    }

    /// 上半格交叉渐变不能染色两端都留白的下半格；空格的隐藏前景色也不能入画。
    #[test]
    fn halfblock_fade_preserves_uncovered_halves() -> color_eyre::Result<()> {
        let area = Rect::new(0, 0, 2, 1);
        let background = Color::Rgb(20, 40, 60);
        let mut from = Buffer::empty(area);
        for cell in &mut from.content {
            cell.set_fg(Color::Rgb(255, 255, 255)).set_bg(background);
        }
        let mut to = from.clone();
        from.cell_mut((0, 0))
            .ok_or_else(|| eyre!("缺少旧图 cell"))?
            .set_char('▀')
            .set_fg(Color::Rgb(200, 0, 0));
        for cell in &mut to.content {
            cell.set_char('▀').set_fg(Color::Rgb(0, 0, 200));
        }
        let mut output = from.clone();
        super::blend_halfblocks(&mut output, &from, &to, 500);
        assert_eq!(
            output
                .content
                .iter()
                .map(|cell| cell.fg)
                .collect::<Vec<_>>(),
            [Color::Rgb(100, 0, 100), Color::Rgb(10, 20, 130)]
        );
        assert!(output.content.iter().all(|cell| cell.bg == background));
        Ok(())
    }
}
