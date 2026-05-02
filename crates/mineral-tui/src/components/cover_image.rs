//! 封面渲染分发:有真图 → ratatui-image stateful protocol,缺失 / 拉失败 /
//! 无 cover_url → 程序化封面。
//!
//! prefetch 触发逻辑在 [`crate::prefetch`] 模块。
//!
//! `StatefulProtocol` cache 的好处:kitty 协议下"图传一次记 id 后续按 id 重画",
//! 重发图等于重新分配 id + 流大 base64,体感变慢且终端积累状态。共享 cached 状态
//! 能避免每帧重发。

use mineral_model::MediaUrl;
use ratatui::layout::Rect;
use ratatui::Frame;
use ratatui_image::picker::Picker;
use ratatui_image::Resize;
use ratatui_image::StatefulImage;

use crate::components::cover;
use crate::state::AppState;
use crate::theme::Theme;

/// 优先 ratatui-image 真图;cache miss / 无 url / 协议不支持时,回退到
/// `crate::components::cover::render` 的程序化封面。
pub fn render_or_fallback(
    frame: &mut Frame<'_>,
    area: Rect,
    cover_url: Option<&MediaUrl>,
    state: &AppState,
    picker: &Picker,
    theme: &Theme,
    fallback_seed: &str,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // 1:1 方图按 cell 像素比算出「视觉正方」sub-area:横向铺满,高度等比锁定。
    // 实图和程序化 fallback 走同一处理,免比例不一致闪。
    let target = square_subarea(area, picker.font_size());
    if target.width == 0 || target.height == 0 {
        return;
    }
    let Some(url) = cover_url else {
        cover::render(frame, target, fallback_seed, theme);
        return;
    };
    let Some(image) = state.cover_cache.get(url).cloned() else {
        cover::render(frame, target, fallback_seed, theme);
        return;
    };
    // 借 cell mut 拿到 / 建 stateful protocol。dims 跟上次渲染不一致时重建,
    // 避免 cache 的 protocol 按旧 dims 编码导致溢出 / 截断。
    let mut protocols = state.cover_protocols.borrow_mut();
    let dims = (target.width, target.height);
    // 滚动防抖:protocol 不在 cache 或 dims 变了都得重建(decode + base64/kitty
    // 编码,render 线程上是百毫秒级开销)。如果用户还在快速 nav,**整个 cover
    // 区留空**(连程序化封面都不画)—— 视觉上就是「滚的时候右栏图位空着」,稳
    // 定 ≥ COVER_DEBOUNCE 后真图淡入。避开「每按一次 j 都重新编码全图」的卡顿,
    // 同时不闪烁程序化色块。
    let needs_build = protocols.get(url).is_none_or(|e| e.1 != dims);
    if needs_build && state.is_scrolling() {
        return;
    }
    let entry = protocols
        .entry(url.clone())
        .or_insert_with(|| (picker.new_resize_protocol((*image).clone()), dims));
    if entry.1 != dims {
        *entry = (picker.new_resize_protocol((*image).clone()), dims);
    }
    // Scale 模式:即使源图 < area px 也会按 area 重采样铺满(Fit 不会向上放大)。
    // target 已是视觉正方 + source 是方图,Scale 不会真的拉伸变形。
    let widget =
        StatefulImage::default().resize(Resize::Scale(Some(image::imageops::FilterType::Triangle)));
    frame.render_stateful_widget(widget, target, &mut entry.0);
}

/// 在 `area` 内算出「视觉正方形」的 sub-area:按 cell 像素比把方图横向铺满。
///
/// 推导:可视宽度 = `cells_w * px_w`,可视高度 = `cells_h * px_h`;方图要求两者相等,
/// 解出 `cells_h = cells_w * px_w / px_h`。若超出 area 高度,反过来按 area 高度算
/// `cells_w`,横向居中(用户面板特别扁的退化情况)。
fn square_subarea(area: Rect, cell_px: (u16, u16)) -> Rect {
    let cw = u32::from(cell_px.0).max(1);
    let ch = u32::from(cell_px.1).max(1);
    let max_h_for_full_w = u16::try_from(u32::from(area.width) * cw / ch).unwrap_or(area.height);
    if max_h_for_full_w <= area.height {
        Rect::new(area.x, area.y, area.width, max_h_for_full_w.max(1))
    } else {
        let w = u16::try_from(u32::from(area.height) * ch / cw)
            .unwrap_or(area.width)
            .min(area.width)
            .max(1);
        let pad = (area.width - w) / 2;
        Rect::new(area.x + pad, area.y, w, area.height)
    }
}
