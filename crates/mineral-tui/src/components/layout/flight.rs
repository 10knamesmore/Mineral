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

use crate::components::layout::shared::transform::{lerp_rect, zero_center};
use crate::components::layout::shared::vinyl;
use crate::image::{BlendStyle, ImageContent, ImageNeeds, ImageRenderPhase, ReadyImages};
use crate::render::blit;
use crate::render::color::lerp_color;
use crate::render::theme::Theme;

/// 飞行端点的可绘制内容；唱片仅用于没有在播曲的全屏端。
pub(crate) enum FlightContent {
    /// 已解码的真实封面。
    Cover(MediaUrl),

    /// 使用当前主题和旋转相位绘制的待机唱片。
    Vinyl,
}

/// 飞行一端：端点稳态的区域与内容。
pub(crate) struct FlightEnd {
    /// 端点稳态区域，正方几何与对应内容的稳态绘制一致。
    pub(crate) area: Rect,

    /// 要显示的封面或唱片。
    pub(crate) content: FlightContent,
}

/// 一次 page morph 的封面飞行计划:两端至少一端就绪。
pub(crate) struct FlightPlan {
    /// browse 端(进度 0 端):now_playing 主封面。
    pub(crate) from: Option<FlightEnd>,

    /// 进度 1000 端：搜索详情封面、全屏在播封面或待机唱片。
    pub(crate) to: Option<FlightEnd>,
}

/// 画一帧飞行层(叠在面板之上):双端 fade 合成、单端独图收放，读取准备阶段保留的图片。
///
/// # Params:
///   - `plan`: [`FlightPlan`] 产出的飞行计划
///   - `progress`: 已缓动千分比，0 为浏览端、1000 为搜索或全屏端；反向沿用同一进度
///   - `state`: 图片缓存与终端成品状态
///   - `theme`: 待机唱片本帧使用的主题
pub(crate) fn render(
    frame: &mut Frame<'_>,
    plan: &FlightPlan,
    progress: u16,
    images: ReadyImages<'_>,
    spin: &vinyl::VinylSpin,
    theme: &Theme,
) {
    let square = |end: &FlightEnd| match &end.content {
        FlightContent::Cover(_) => images.square_area(end.area),
        FlightContent::Vinyl => crate::image::square_cells(end.area),
    };
    match (&plan.from, &plan.to) {
        (Some(from), Some(to)) => {
            if let (FlightContent::Cover(from_url), FlightContent::Cover(to_url)) =
                (&from.content, &to.content)
            {
                let rect = lerp_rect(square(from), square(to), progress);
                images.render(
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
                render_end(&mut old, rect, &from.content, images, spin, theme);
                render_end(&mut new, rect, &to.content, images, spin, theme);
                blend_halfblocks(screen, &old, &new, progress);
            }
        }
        // 单端:图沿自身中心收缩 / 生长(与消失面板的 collapse 语义同款),不强行合成。
        (Some(from), None) => {
            let sq = square(from);
            let rect = lerp_rect(sq, zero_center(sq), progress);
            render_end(frame.buffer_mut(), rect, &from.content, images, spin, theme);
        }
        (None, Some(to)) => {
            let sq = square(to);
            let rect = lerp_rect(zero_center(sq), sq, progress);
            render_end(frame.buffer_mut(), rect, &to.content, images, spin, theme);
        }
        (None, None) => {}
    }
}

/// 飞行两端的图片在准备阶段保活，并按各自稳定尺寸预编码。
pub(crate) fn prepare(plan: &FlightPlan, images: &mut ImageNeeds<'_>) {
    for end in [plan.from.as_ref(), plan.to.as_ref()].into_iter().flatten() {
        if let FlightContent::Cover(url) = &end.content {
            images.display(Some(url), end.area, ImageRenderPhase::Resizing);
            images.prewarm(url, end.area);
        }
    }
}

/// 两端各自按同一移动区域绘制，封面保留原图比例，唱片沿用当前旋转相位。
fn render_end(
    buf: &mut Buffer,
    area: Rect,
    content: &FlightContent,
    images: ReadyImages<'_>,
    spin: &vinyl::VinylSpin,
    theme: &Theme,
) {
    match content {
        FlightContent::Cover(url) => images.render(
            ImageContent::Display { url: Some(url) },
            area,
            buf,
            ImageRenderPhase::Resizing,
        ),
        FlightContent::Vinyl => vinyl::render_to(buf, area, spin, theme),
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
            crate::view::flight::plan(&normal, &search, &app.state).is_none(),
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
        let plan = crate::view::flight::plan(&normal, &search, &app.state)
            .ok_or_else(|| eyre!("browse 端图已缓存,应有单端计划"))?;
        assert!(plan.from.is_some(), "browse 端应就绪");
        assert!(plan.to.is_none(), "detail 端图未缓存应缺席");
        Ok(())
    }
}
