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
    let Some(url) = cover_url else {
        cover::render(frame, area, fallback_seed, theme);
        return;
    };
    let Some(image) = state.cover_cache.get(url).cloned() else {
        cover::render(frame, area, fallback_seed, theme);
        return;
    };
    // 借 cell mut 拿到 / 建 stateful protocol;首次访问时按 picker 探测的最优协议建。
    let mut protocols = state.cover_protocols.borrow_mut();
    let proto = protocols
        .entry(url.clone())
        .or_insert_with(|| picker.new_resize_protocol((*image).clone()));
    let widget = StatefulImage::default().resize(Resize::Fit(None));
    frame.render_stateful_widget(widget, area, proto);
}
