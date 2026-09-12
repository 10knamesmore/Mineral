//! 详情下钻与返回过渡：把出发帧和目标帧离屏绘制，再按方向、风格与缓动进度合成。

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::frame::render_frame_to;
use super::sweep::{FULL, SweepLayer, copy_col, sweep_column};
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, DetailFrame};

/// 下钻/返回滑动的合成参数：出发/目标帧 + 缓动进度 + 方向（打包避免 `draw_sweep` 参数过多）。
#[derive(Clone, Copy)]
pub(super) struct SweepArgs<'a> {
    /// 出发帧（滑出）。
    pub(super) from: &'a DetailFrame,

    /// 目标帧（滑入）。
    pub(super) to: &'a DetailFrame,

    /// 缓动后进度（千分比）。
    pub(super) eased: u16,

    /// 方向：`true` = 下钻右入、`false` = 返回左入。
    pub(super) is_push: bool,

    /// page morph 封面飞行层已接管头图（离屏帧同样跳过自画防双画）。
    pub(super) cover_in_flight: bool,
}

/// 下钻/返回滑动：出发帧与目标帧各渲染到离屏 Buffer，按 `eased` 列合成。风格随配置
/// `view_sweep`（经 [`sweep_column`]），与 artist 双区切换 / 左栏视图切换同款；方向由 `is_push`
/// 定（下钻目标右入、返回左入）。
pub(super) fn draw_sweep(
    frame: &mut Frame<'_>,
    inner: Rect,
    args: SweepArgs<'_>,
    state: &AppState,
    theme: &Theme,
) {
    let SweepArgs {
        from,
        to,
        eased,
        is_push,
        cover_in_flight,
    } = args;
    let mut from_buf = Buffer::empty(inner);
    let mut to_buf = Buffer::empty(inner);
    render_frame_to(&mut from_buf, inner, from, state, theme, cover_in_flight);
    render_frame_to(&mut to_buf, inner, to, state, theme, cover_in_flight);
    let w = inner.width;
    let advance = u16::try_from(u32::from(w) * u32::from(eased) / FULL)
        .unwrap_or(w)
        .min(w);
    let style = *state.cfg.tui().animation().view_sweep();
    let buf = frame.buffer_mut();
    for c in 0..w {
        let (src, src_c) = match sweep_column(style, is_push, c, w, advance) {
            (SweepLayer::From, src_c) => (&from_buf, src_c),
            (SweepLayer::To, src_c) => (&to_buf, src_c),
        };
        copy_col(buf, inner, src, c, src_c);
    }
}
