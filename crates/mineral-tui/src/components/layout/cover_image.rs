//! 封面渲染分发:命中已编码协议 → place 真图;缺失 / 拉失败 / 无 cover_url → 程序化封面。
//!
//! prefetch(拉图)触发逻辑在 [`crate::runtime::prefetch`];resize + kitty 编码**不在此处
//! 同步做**,而是投递给 [`crate::runtime::cover_encode::CoverEncoder`] 的 worker 离线跑,
//! 渲染线程只命中已编码协议直接 place(`StatefulProtocol` 内部记 kitty image id,同尺寸
//! 渲染只重发占位符、不重编码)。把百毫秒级的 resize/base64 挪出渲染线程,切歌 / 关浮层不卡帧。

use std::sync::Arc;

use image::DynamicImage;
use mineral_model::MediaUrl;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui_image::Resize;
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;

use crate::components::layout::cover;
use crate::render::theme::Theme;
use crate::runtime::cover_encode::EncodeRequest;
use crate::runtime::state::AppState;

/// 优先 ratatui-image 真图;cache miss / 无 url / 协议不支持时,回退到
/// `crate::components::layout::cover::render` 的程序化封面。
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
    // 全屏形变期:此处的唯一调用方是正在收缩 / 生长的 now_playing 消失面板,其尺寸逐帧
    // 变。若在形变中驱动有状态封面协议,会每帧按新 dims `new_resize_protocol` 重建 ——
    // kitty 每次重建分配新 image id 并在移动位置 transmit,稳态落地后占位符指向的 id 已被
    // 后续帧覆盖 / 终端不再持有,封面整块空白且按 url 粘死(重选同曲 dims 不变不再重建,
    // 永不重发)。形变期一律让位给 `draw_fullscreen_cover` 的程序化封面,真图只在两端稳态
    // 渲染。
    if !state.fullscreen_pos.settled() {
        return;
    }
    let Some(image) = state.cover_cache.get(url).cloned() else {
        // 还没拉到图:程序化占位(fetch worker 完成后进 cover_cache,后续帧再走编码)。
        cover::render(frame, target, fallback_seed, theme);
        return;
    };
    let dims = (target.width, target.height);
    // 命中已编码协议(同尺寸)→ 直接 place。`StatefulProtocol` 内部记着 kitty image id,
    // 同尺寸渲染 `needs_resize` 返回 `None`,只重发占位符不重编码,渲染线程零开销。
    {
        let mut protocols = state.cover_protocols.borrow_mut();
        if let Some(entry) = protocols.get_mut(url)
            && entry.1 == dims
        {
            let widget = StatefulImage::default()
                .resize(Resize::Scale(Some(image::imageops::FilterType::Triangle)));
            frame.render_stateful_widget(widget, target, &mut entry.0);
            return;
        }
    }
    // 未命中(无缓存协议 / 尺寸变了):**不在渲染线程编码**(resize + base64 是百毫秒级,
    // 会卡帧),改投递给 [`CoverEncoder`] 的 worker 离线编码。
    //
    // - 滚动中:留空,既不闪程序化色块也不投递 —— 避免给 worker 灌一堆滚过即弃的图,
    //   稳定 ≥ cover.debounce_ms 后再编码淡入(沿用旧的「滚时图位空着」体感)。
    // - 稳定后:按 `(url, dims)` 去重投递一次,在途期间画程序化占位;worker 完成后主循环
    //   `drain_ready_protocols` 装回 `cover_protocols`,下一帧命中上真图。
    if state.is_scrolling() {
        return;
    }
    request_cover_encode(state, url, image, target);
    cover::render(frame, target, fallback_seed, theme);
}

/// 未命中已编码协议时,按 `(url, dims)` 去重投递一次离线编码请求(`image` 来自
/// `cover_cache`)。worker 完成后主循环 `drain_ready_protocols` 装回 `cover_protocols`。
fn request_cover_encode(state: &AppState, url: &MediaUrl, image: Arc<DynamicImage>, target: Rect) {
    let key = (url.clone(), (target.width, target.height));
    if state.cover_encode_pending.borrow_mut().insert(key) {
        let _ = state.cover_encode_tx.send(EncodeRequest {
            url: url.clone(),
            image,
            target,
        });
    }
}

/// 预热一张封面:把 `url`(图须已在 `cover_cache`)在 `area` 对应的封面尺寸下**提前编码**,
/// 使其真正渲染时协议已就绪、直接 place 无闪。已编码(同尺寸)/ 图未就绪 → 无操作,不渲染。
///
/// 仅供尺寸稳定的场景调用(全屏稳态封面区固定)——预编码的 dims 须与目标真正渲染时一致才命中。
///
/// # Params:
///   - `area`: 目标封面区域(与真正渲染处同一 `area`,内部走相同的视觉正方换算)
///   - `url`: 待预热封面 URL
pub fn prewarm(state: &AppState, picker: &Picker, area: Rect, url: &MediaUrl) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let target = square_subarea(area, picker.font_size());
    if target.width == 0 || target.height == 0 {
        return;
    }
    let dims = (target.width, target.height);
    if matches!(state.cover_protocols.borrow().get(url), Some(e) if e.1 == dims) {
        return;
    }
    let Some(image) = state.cover_cache.get(url).cloned() else {
        return;
    };
    request_cover_encode(state, url, image, target);
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
