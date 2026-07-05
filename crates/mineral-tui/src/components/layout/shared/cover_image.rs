//! 封面渲染分发:命中已编码协议 → place 真图;缺失 / 拉失败 / 无 cover_url → 程序化封面。
//!
//! prefetch(拉图)触发逻辑在 [`crate::runtime::prefetch`];resize + kitty 编码**不在此处
//! 同步做**,而是投递给 [`crate::runtime::cover::encode::CoverEncoder`] 的 worker 离线跑,
//! 渲染线程只命中已编码协议直接 place(`StatefulProtocol` 内部记 kitty image id,同尺寸
//! 渲染只重发占位符、不重编码)。把百毫秒级的 resize/base64 挪出渲染线程,切歌 / 关浮层不卡帧。

use std::sync::Arc;

use image::{DynamicImage, Rgb};
use mineral_model::MediaUrl;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui_image::Resize;
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;

use crate::components::layout::shared::cover;
use crate::render::theme::Theme;
use crate::runtime::cover::encode::EncodeRequest;
use crate::runtime::state::AppState;

/// 优先 ratatui-image 真图;cache miss / 无 url / 协议不支持时,回退到
/// `crate::components::layout::shared::cover::render` 的程序化封面。
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
    // 全局布局形变期：绝不上 `StatefulProtocol` 真图(那会触发 kitty 逐帧重编码 churn /
    // 占位符指向被覆盖 id 的整块粘死),但**可以**用 halfblock 真图——它是纯终端 cell、无带外
    // image-id,逐帧重画安全(见 `render_halfblock_to`)。命中缓存即画低清真图随形变长大,
    // 落定稳态再 snap 成下方 kitty/sixel 高清图;未命中则留空(等 fetch,不闪程序化)。
    // fullscreen / channel-search 任一未 settle 都走此路。
    if !state.browse.fullscreen.settled() || !state.channel_search.active.settled() {
        if let Some(image) = state.covers.cache.get(url) {
            render_halfblock_to(frame.buffer_mut(), target, image);
        }
        return;
    }
    let Some(image) = state.covers.cache.get(url).cloned() else {
        // 还没拉到图:程序化占位(fetch worker 完成后进 covers.cache,后续帧再走编码)。
        cover::render(frame, target, fallback_seed, theme);
        return;
    };
    let dims = (target.width, target.height);
    // 命中已编码协议(同尺寸)→ 直接 place。`StatefulProtocol` 内部记着 kitty image id,
    // 同尺寸渲染 `needs_resize` 返回 `None`,只重发占位符不重编码,渲染线程零开销。
    // 命中即标为最近渲染(LRU touch),防止正在显示的协议被字节预算逐出。
    let placed = state.covers.protocols.render_hit(url, dims, |protocol| {
        let widget = StatefulImage::default()
            .resize(Resize::Scale(Some(image::imageops::FilterType::Triangle)));
        frame.render_stateful_widget(widget, target, protocol);
    });
    if placed {
        return;
    }
    // 未命中(无缓存协议 / 尺寸变了):**不在渲染线程编码**(resize + base64 是百毫秒级,
    // 会卡帧),改投递给 [`CoverEncoder`] 的 worker 离线编码。
    //
    // - 滚动中:留空,既不闪程序化色块也不投递 —— 避免给 worker 灌一堆滚过即弃的图,
    //   稳定 ≥ cover.debounce_ms 后再编码淡入(沿用旧的「滚时图位空着」体感)。
    // - 稳定后:按 `(url, dims)` 去重投递一次,**在途期间画 halfblock 真图**(非程序化 hash)——
    //   手里已有解好的 `image`,kitty 大图编码要好几帧,这期间退 hash 会在「切到全屏落定瞬间」
    //   闪一下色块;halfblock 让封面从形变→编码等待→kitty 全程都是真图,worker 完成后主循环
    //   `drain_ready_protocols` 装回 `covers.protocols`,下一帧 snap 成 crisp kitty。三态同渲于
    //   `target`,零位移。`fallback_seed` 仅当无图(上方早退)时才用,此处图必在故不取。
    if state.is_scrolling() {
        return;
    }
    render_halfblock_to(frame.buffer_mut(), target, &image);
    request_cover_encode(state, picker, url, image, target);
}

/// 未命中已编码协议时,按 `(url, dims)` 去重投递一次离线编码请求(`image` 来自
/// `covers.cache`)。worker 完成后主循环 `drain_ready_protocols` 装回 `covers.protocols`。
/// `picker` 随请求携带(字号是编码尺寸换算的分母,终端字号变化后须用当前值)。
fn request_cover_encode(
    state: &AppState,
    picker: &Picker,
    url: &MediaUrl,
    image: Arc<DynamicImage>,
    target: Rect,
) {
    let key = (url.clone(), (target.width, target.height));
    if state.covers.encode_pending.borrow_mut().insert(key) {
        let _ = state.covers.encode_tx.send(EncodeRequest {
            url: url.clone(),
            image,
            target,
            picker: picker.clone(),
        });
    }
}

/// 预热一张封面:把 `url`(图须已在 `covers.cache`)在 `area` 对应的封面尺寸下**提前编码**,
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
    if state.covers.protocols.contains_dims(url, dims) {
        return;
    }
    let Some(image) = state.covers.cache.get(url).cloned() else {
        return;
    };
    request_cover_encode(state, picker, url, image, target);
}

/// 把已解码真图按 halfblock(`▀` 半字符)逐 cell 画进 `area`(**精确铺满**,不再内部正方化——
/// 正方区由调用方算好再传入)。每 cell:上半像素 → fg、下半像素 → bg;源图先 `resize_exact`
/// 到 `area.width × area.height*2` 像素再逐 cell 采样。
///
/// 纯写终端 cell、不碰终端图协议(kitty image-id / sixel 缓冲),故**形变期逐帧重画安全**——
/// 这正是它能在 [`render_or_fallback`] 形变早退处替真图出场、而 `StatefulProtocol` 真图不能的原因。
/// 降采样在渲染线程同步做:源图 ≤ 384px、目标几十像素,Triangle 一次亚毫秒级。
///
/// # Params:
///   - `buf`: 目标缓冲(屏上 / 离屏皆可)
///   - `area`: 铺图区域(宽高任一为 0 直接返回)
///   - `image`: 已解码封面原图
pub fn render_halfblock_to(buf: &mut Buffer, area: Rect, image: &DynamicImage) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let px_w = u32::from(area.width);
    let px_h = u32::from(area.height).saturating_mul(2);
    let small = image
        .resize_exact(px_w, px_h, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let sample = |x: u32, y: u32| -> Color {
        small.get_pixel_checked(x, y).map_or(Color::Reset, |p| {
            let Rgb([r, g, b]) = *p;
            Color::Rgb(r, g, b)
        })
    };
    for cy in 0..area.height {
        let py = u32::from(cy).saturating_mul(2);
        for cx in 0..area.width {
            let px = u32::from(cx);
            let style = Style::new()
                .fg(sample(px, py))
                .bg(sample(px, py.saturating_add(1)));
            buf.set_string(area.x + cx, area.y + cy, "▀", style);
        }
    }
}

/// 形变期 morph-safe 封面(屏上,字号版):命中缓存真图 → halfblock 真图(`square_subarea`
/// 按字号锁正方,与稳态 kitty 落点一致,落定不跳);否则 → 程序化色块。**不碰 `StatefulProtocol`**,
/// 故逐帧重画安全。用于 fullscreen 形变中途。
///
/// # Params:
///   - `cover_url`: 在播 / 当前实体封面 URL(`None` 直接程序化)
///   - `fallback_seed`: 无图时程序化封面的种子(专辑名 / 歌名)
pub fn render_morph(
    frame: &mut Frame<'_>,
    area: Rect,
    cover_url: Option<&MediaUrl>,
    state: &AppState,
    picker: &Picker,
    theme: &Theme,
    fallback_seed: &str,
) {
    if let Some(image) = cover_url.and_then(|url| state.covers.cache.get(url)) {
        let target = square_subarea(area, picker.font_size());
        if target.width != 0 && target.height != 0 {
            render_halfblock_to(frame.buffer_mut(), target, image);
            return;
        }
    }
    cover::render(frame, area, fallback_seed, theme);
}

/// [`render_morph`] 的离屏 [`Buffer`] 版(无 `Picker`):命中缓存真图 → halfblock 真图
/// (`cover::square_cells` 正方化,与同处程序化占位几何一致);否则 → 程序化色块。
/// 用于 detail 下钻 / 返回 sweep 的离屏合成(出发 / 目标帧各画一份头图再列混合)。
///
/// # Params:
///   - `cover_url`: 该帧实体封面 URL(`None` 直接程序化)
///   - `fallback_seed`: 无图时程序化封面的种子
pub fn render_morph_to(
    buf: &mut Buffer,
    area: Rect,
    cover_url: Option<&MediaUrl>,
    state: &AppState,
    theme: &Theme,
    fallback_seed: &str,
) {
    if let Some(image) = cover_url.and_then(|url| state.covers.cache.get(url)) {
        let target = cover::square_cells(area);
        if target.width != 0 && target.height != 0 {
            render_halfblock_to(buf, target, image);
            return;
        }
    }
    cover::render_to(buf, area, fallback_seed, theme);
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

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use image::{DynamicImage, Rgb, RgbImage};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use super::render_halfblock_to;

    /// 纯色图降采样:每个 cell 都是 `▀`,fg/bg 同为该色 —— 均匀图无边缘,Triangle 重采样不改色,
    /// 故期望色可精确断言。
    #[test]
    fn halfblock_uniform_image_fills_solid() -> color_eyre::Result<()> {
        let mut img = RgbImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = Rgb([200, 50, 50]);
        }
        let image = DynamicImage::ImageRgb8(img);
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);

        render_halfblock_to(&mut buf, area, &image);

        for y in 0..2u16 {
            for x in 0..4u16 {
                let cell = buf
                    .cell((x, y))
                    .ok_or_else(|| eyre!("cell ({x},{y}) 越界"))?;
                assert_eq!(cell.symbol(), "▀", "cell ({x},{y}) 应为上半字符");
                assert_eq!(
                    cell.fg,
                    Color::Rgb(200, 50, 50),
                    "cell ({x},{y}) 上半像素色"
                );
                assert_eq!(
                    cell.bg,
                    Color::Rgb(200, 50, 50),
                    "cell ({x},{y}) 下半像素色"
                );
            }
        }
        Ok(())
    }

    /// 上半红 / 下半蓝:顶 cell 取顶部像素(红)、底 cell 取底部像素(蓝)—— 证明采样的是真图,
    /// 而非退回程序化 hash 色块。中间 cell 跨红蓝边界会混色,只断言远离边界的顶 / 底 cell。
    #[test]
    fn halfblock_samples_top_and_bottom() -> color_eyre::Result<()> {
        let mut img = RgbImage::new(4, 16);
        for (_x, y, p) in img.enumerate_pixels_mut() {
            *p = if y < 8 {
                Rgb([220, 0, 0])
            } else {
                Rgb([0, 0, 220])
            };
        }
        let image = DynamicImage::ImageRgb8(img);
        let area = Rect::new(0, 0, 4, 4);
        let mut buf = Buffer::empty(area);

        render_halfblock_to(&mut buf, area, &image);

        let top = buf.cell((0, 0)).ok_or_else(|| eyre!("顶 cell 越界"))?;
        assert_eq!(top.fg, Color::Rgb(220, 0, 0), "顶 cell 上半 = 红");
        let bottom = buf.cell((0, 3)).ok_or_else(|| eyre!("底 cell 越界"))?;
        assert_eq!(bottom.bg, Color::Rgb(0, 0, 220), "底 cell 下半 = 蓝");
        Ok(())
    }
}
