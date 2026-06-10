//! 平铺界面的整屏几何变换动画:一个整 cell 边框在区域内按千分比进度朝锚点缩放,
//! 框内保留已画内容、框外清成背景色。启动扩大与退出收缩共用此实现,缩放方向只取决于
//! 进度走向,渲染本身方向无关。

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear};

use crate::components::layout::compute::Areas;
use crate::render::anim::lerp_u16;
use crate::render::theme::Theme;

/// 收缩进度满值(千分比),对齐 [`crate::render::anim::Transition::eased`] 的满值。
const FULL_SCALE: u32 = 1000;

/// 整屏缩放裁剪:一个整 cell 边框在 `area` 内按 `scale`(千分比)朝 `anchor` 缩放,框内保留
/// 已画的界面内容(静止),框外清成背景色。`scale` 由 0 涨到满即启动扩大、由满收到 0 即退出
/// 收缩 —— 方向只取决于 `scale` 的走向,渲染本身方向无关。
///
/// `anchor` 为进 alternate screen 前捕获的光标位置:expand 从此点铺开、collapse 收回此点。
/// `None`(无 TTY)时退化回屏幕居中,即历史行为。
pub fn clip_scaled(
    frame: &mut Frame<'_>,
    area: Rect,
    scale: u16,
    anchor: Option<Position>,
    theme: &Theme,
) {
    let scale = u32::from(scale);
    let w = u16::try_from(u32::from(area.width) * scale / FULL_SCALE).unwrap_or(area.width);
    let h = u16::try_from(u32::from(area.height) * scale / FULL_SCALE).unwrap_or(area.height);

    let area_bottom = area.y.saturating_add(area.height);
    let area_right = area.x.saturating_add(area.width);
    // 框左上角定位:有锚点则朝锚点插值,无锚点(无 TTY)退化回居中。
    // 居中分支逐字节沿用历史公式(`(area - 框) / 2`),与定点取整顺序一致 → 历史快照不变;
    // 锚点分支才走新插值,把改动收窄到「确有光标位置」时。
    let (x, y) = match anchor {
        Some(p) => {
            // clamp 进 area(防 enter 后终端缩小致 row/col 越界);`.max(area.{x,y})` 保证
            // clamp 的 min ≤ max(空 area 时也不 panic)。
            let ax = p.x.clamp(area.x, area_right.saturating_sub(1).max(area.x));
            let ay = p.y.clamp(area.y, area_bottom.saturating_sub(1).max(area.y));
            // scale=满 → 贴 area 原点(全屏);scale=0 → 收到锚点(成一点)。锚点在框内
            // 相对比例恒定 → 真正"从该点放 / 朝该点收"。全程非负定点,不碰 as 强转。
            let dx = u32::from(ax - area.x) * (FULL_SCALE - scale) / FULL_SCALE;
            let dy = u32::from(ay - area.y) * (FULL_SCALE - scale) / FULL_SCALE;
            (
                area.x.saturating_add(u16::try_from(dx).unwrap_or(0)),
                area.y.saturating_add(u16::try_from(dy).unwrap_or(0)),
            )
        }
        None => (
            area.x + area.width.saturating_sub(w) / 2,
            area.y + area.height.saturating_sub(h) / 2,
        ),
    };

    // 框外四条 bar 清成背景色,把界面"吞掉"。
    let below = y.saturating_add(h);
    let right = x.saturating_add(w);
    fill_bg(
        frame,
        Rect::new(area.x, area.y, area.width, y.saturating_sub(area.y)),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(area.x, below, area.width, area_bottom.saturating_sub(below)),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(area.x, y, x.saturating_sub(area.x), h),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(right, y, area_right.saturating_sub(right), h),
        theme.base,
    );

    // 收缩边框(只画线,不填内部 → 保留框内内容);收到太小画不出框就只剩清屏。
    if w >= 2 && h >= 2 {
        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.accent));
        frame.render_widget(block, Rect::new(x, y, w, h));
    }
}

/// 把 `rect` 清成纯 `color` 背景(先 `Clear` 去掉旧字符,再铺底色)。空矩形跳过。
fn fill_bg(frame: &mut Frame<'_>, rect: Rect, color: Color) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    frame.render_widget(Clear, rect);
    frame.render_widget(Block::new().style(Style::new().bg(color)), rect);
}

/// 在常规与全屏两套 [`Areas`] 间按进度 `t`(eased_in_out 千分比 `0..=1000`)逐 rect 插值,
/// 得到形变中途的布局。各面板语义:
///   - 消失面板(`top_status` / `left` / `right`):朝自身中心收缩到零面积(原地收掉);
///   - `cover` / `lyrics` / `spectrum`:常规位 → 全屏位(常规缺位则从全屏位收缩到零,即「从无长出」);
///   - `transport`:常规位 → 全屏位。
///
/// # Params:
///   - `normal`: 常规布局([`compute`](crate::components::layout::compute::compute) 产出)
///   - `full`: 全屏布局([`compute_fullscreen`](crate::components::layout::compute::compute_fullscreen) 产出)
///   - `t`: 形变进度,千分比 `0..=1000`(取 [`Transition::eased_in_out`](crate::render::anim::Transition::eased_in_out))
///
/// # Return:
///   中途布局的 [`Areas`]。
pub fn morph_areas(normal: &Areas, full: &Areas, t: u16) -> Areas {
    // 消失面板:朝自身中心收缩到零(原地收掉)。
    let collapse = |r: Rect| lerp_rect(r, zero_center(r), t);
    let right_start = normal.right.unwrap_or_else(|| zero_center(normal.left));
    // 保留面板:常规位(缺则从全屏位收缩到零)→ 全屏位。
    let grow = |normal_rect: Option<Rect>, full_rect: Option<Rect>| {
        full_rect.map(|fr| lerp_rect(grow_start(normal_rect, fr), fr, t))
    };
    Areas {
        mode: full.mode,
        top_status: collapse(normal.top_status),
        left: collapse(normal.left),
        right: Some(collapse(right_start)),
        cover: grow(normal.cover, full.cover),
        lyrics: grow(normal.lyrics, full.lyrics),
        spectrum: grow(normal.spectrum, full.spectrum),
        transport: lerp_rect(normal.transport, full.transport, t),
    }
}

/// 保留面板的形变起点:有常规位从常规位出发,无则从全屏位收缩到零(「从无长出」)。
fn grow_start(normal: Option<Rect>, full: Rect) -> Rect {
    normal.unwrap_or_else(|| zero_center(full))
}

/// 把矩形退化为「以自身中心为原点的零面积矩形」,作形变收缩 / 长出端点。
fn zero_center(r: Rect) -> Rect {
    Rect::new(
        r.x.saturating_add(r.width / 2),
        r.y.saturating_add(r.height / 2),
        0,
        0,
    )
}

/// 逐 x/y/w/h 在两矩形间按千分比 `t` 定点插值。
fn lerp_rect(a: Rect, b: Rect, t: u16) -> Rect {
    Rect::new(
        lerp_u16(a.x, b.x, t),
        lerp_u16(a.y, b.y, t),
        lerp_u16(a.width, b.width, t),
        lerp_u16(a.height, b.height, t),
    )
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{lerp_rect, morph_areas, zero_center};
    use crate::components::layout::compute::{compute, compute_fullscreen};

    /// `lerp_rect` 两端点回到 a / b,中点逐字段取中。
    #[test]
    fn lerp_rect_endpoints_and_mid() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(20, 20, 30, 30);
        assert_eq!(lerp_rect(a, b, /*t*/ 0), a);
        assert_eq!(lerp_rect(a, b, /*t*/ 1000), b);
        assert_eq!(lerp_rect(a, b, /*t*/ 500), Rect::new(10, 10, 20, 20));
    }

    /// `zero_center` 收成「自身中心点 + 零面积」。
    #[test]
    fn zero_center_is_center_point() {
        assert_eq!(zero_center(Rect::new(4, 6, 10, 8)), Rect::new(9, 10, 0, 0));
    }

    /// 形变端点:t=0 各保留面板回常规位、t=1000 抵达全屏位,消失面板在全屏端收成零面积。
    #[test]
    fn morph_endpoints_match_normal_and_full() -> color_eyre::Result<()> {
        let cfg = mineral_config::Config::defaults()?.tui().layout().clone();
        let area = Rect::new(0, 0, 100, 40);
        let normal = compute(area, &cfg);
        let full = compute_fullscreen(area, &cfg);

        let m0 = morph_areas(&normal, &full, /*t*/ 0);
        assert_eq!(m0.transport, normal.transport, "t=0 transport 回常规位");
        assert_eq!(m0.lyrics, normal.lyrics, "t=0 lyrics 回常规位");
        assert_eq!(m0.cover, normal.cover, "t=0 cover 回常规锚点");

        let m1 = morph_areas(&normal, &full, /*t*/ 1000);
        assert_eq!(m1.transport, full.transport, "t=1000 transport 抵全屏位");
        assert_eq!(m1.cover, full.cover, "t=1000 cover 抵全屏左半");
        assert_eq!(m1.lyrics, full.lyrics, "t=1000 lyrics 抵全屏右半");
        assert_eq!(m1.left.width, 0, "t=1000 左栏收零宽");
        assert_eq!(m1.left.height, 0, "t=1000 左栏收零高");
        Ok(())
    }
}
