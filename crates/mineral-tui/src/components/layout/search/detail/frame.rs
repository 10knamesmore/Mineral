//! 详情帧的头图、元数据和列表绘制。稳态使用终端图片，离屏帧使用 halfblock；两者均只读显示状态。

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::body::draw_body;
use super::geometry::{split_frame, split_head};
use super::meta::draw_meta;
use super::placeholder::draw_delimiter;
use crate::image::{ImageContent, ImageRenderPhase};
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, DetailFrame, EntityRef};

/// 稳态一帧：头图走图片引擎，元数据与列表画主帧。
#[allow(clippy::too_many_arguments)] // reason: 纯渲染入口,参数即全部输入,收拢成 struct 反而多一层搬运
pub(super) fn draw_frame_real(
    frame: &mut Frame<'_>,
    inner: Rect,
    dframe: &DetailFrame,
    state: &AppState,
    theme: &Theme,
    show_back: bool,
    cover_in_flight: bool,
) {
    let (head, delim, body) = split_frame(inner);
    let is_artist = matches!(dframe.entity, EntityRef::Artist(_));
    let (cover_a, meta_a, right_a) = split_head(head, is_artist);
    if !cover_in_flight {
        state.images.render(
            ImageContent::Display {
                url: dframe.entity.cover(),
            },
            cover_a,
            frame.buffer_mut(),
            state.image_render_phase(),
        );
    }
    // artist 帧右栏:当前列表选中项封面(随 [ ] 切区 / 光标移动;preview 也未到时留空)。
    if let Some(right_a) = right_a {
        let sel_cover = dframe.selected_cover();
        state.images.render(
            ImageContent::Display { url: sel_cover },
            right_a,
            frame.buffer_mut(),
            state.image_render_phase(),
        );
    }
    let buf = frame.buffer_mut();
    draw_meta(buf, meta_a, dframe, theme, show_back);
    draw_delimiter(buf, delim, theme);
    draw_body(buf, body, dframe, state, theme);
}

/// 把一帧渲染到离屏 Buffer：完整图走 halfblock，否则尝试 preview，均缺失时留空。
pub(super) fn render_frame_to(
    buf: &mut Buffer,
    inner: Rect,
    dframe: &DetailFrame,
    state: &AppState,
    theme: &Theme,
    cover_in_flight: bool,
) {
    let (head, delim, body) = split_frame(inner);
    let is_artist = matches!(dframe.entity, EntityRef::Artist(_));
    let (cover_a, meta_a, right_a) = split_head(head, is_artist);
    if cover_a.width > 0 && !cover_in_flight {
        state.images.render(
            ImageContent::Display {
                url: dframe.entity.cover(),
            },
            cover_a,
            buf,
            ImageRenderPhase::Offscreen,
        );
    }
    if let Some(right_a) = right_a {
        state.images.render(
            ImageContent::Display {
                url: dframe.selected_cover(),
            },
            right_a,
            buf,
            ImageRenderPhase::Offscreen,
        );
    }
    draw_meta(buf, meta_a, dframe, theme, /*show_back*/ false);
    draw_delimiter(buf, delim, theme);
    // 离屏合成(下钻 / 返回滑动期):只读展示当前视口,不推进动画、不改滚动目标。
    draw_body(buf, body, dframe, state, theme);
}
