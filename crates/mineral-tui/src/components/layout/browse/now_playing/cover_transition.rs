//! 在固定封面区域交叉渐变歌单与选中曲目图片，并预热两端稳定尺寸。

use mineral_model::MediaUrl;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::image::{BlendStyle, ImageContent, ImageRenderPhase};
use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, View};

use super::main_cover;

/// 绘制视图切换中的主封面。页面飞行层接管时，调用方必须跳过本函数。
///
/// # Params:
///   - `area`: 详情面板完整区域
///   - `progress`: 与左栏共用的缓动千分比，0 为歌单、1000 为曲目，不随方向交换
pub(super) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    progress: u16,
) {
    let Some([cover_area, _, _]) = main_cover::sections(area) else {
        return;
    };
    let from = main_cover::url_for_view(state, View::Playlists);
    let to = main_cover::url_for_view(state, View::Library);
    let square = state.images.square_area(cover_area);

    if let (Some(from), Some(to)) = (&from, &to)
        && state.images.cache.contains_key(from)
        && state.images.cache.contains_key(to)
    {
        state.images.render(
            ImageContent::Blend {
                from,
                to,
                progress,
                style: BlendStyle::Fade,
                // 视图切换没有推进档位:两端是同一首歌 / 两张不同选中项的封面。
                advance: None,
            },
            square,
            frame.buffer_mut(),
            ImageRenderPhase::Resizing,
        );
    } else {
        // Blend 缺一张完整图时会直接显示目标图；分开绘制可保留真实 preview，
        // 并让没有可用像素的一端渐变到当前背景。
        let from = cover_cells(state, cover_area, from.as_ref());
        let to = cover_cells(state, cover_area, to.as_ref());
        fade_available_cells(frame.buffer_mut(), square, &from, &to, progress, theme);
    }

    for url in [from.as_ref(), to.as_ref()].into_iter().flatten() {
        state.images.prepare(url, cover_area);
    }
}

/// 使用与稳态一致的几何获取真实图片或 preview；无图保持空 cell。
fn cover_cells(state: &AppState, area: Rect, url: Option<&MediaUrl>) -> Buffer {
    let mut cells = Buffer::empty(area);
    state.images.render(
        ImageContent::Display { url },
        area,
        &mut cells,
        ImageRenderPhase::Resizing,
    );
    cells
}

/// 对图片引擎产出的 halfblock 上下像素分别插值；两端都没画图的格子保留背景。
fn fade_available_cells(
    target: &mut Buffer,
    area: Rect,
    from: &Buffer,
    to: &Buffer,
    progress: u16,
    theme: &Theme,
) {
    let area = area.intersection(target.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let from = from.cell((x, y)).filter(|cell| cell.symbol() == "▀");
            let to = to.cell((x, y)).filter(|cell| cell.symbol() == "▀");
            if from.is_none() && to.is_none() {
                continue;
            }
            let Some(cell) = target.cell_mut((x, y)) else {
                continue;
            };
            let background = match cell.bg {
                Color::Reset => theme.base,
                color => color,
            };
            let (from_top, from_bottom) = from.map_or((background, background), |c| (c.fg, c.bg));
            let (to_top, to_bottom) = to.map_or((background, background), |c| (c.fg, c.bg));
            cell.set_symbol("▀")
                .set_fg(lerp_color(from_top, to_top, u64::from(progress), 1000))
                .set_bg(lerp_color(
                    from_bottom,
                    to_bottom,
                    u64::from(progress),
                    1000,
                ));
        }
    }
}
