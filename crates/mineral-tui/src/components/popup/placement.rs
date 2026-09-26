//! PopMenu 的锚定放置算法:纯函数,给定锚点矩形、方向偏好、期望尺寸与屏幕,
//! 算出最终绘制矩形。
//!
//! 规则:首选方向放不下 → 对侧 → Below → Above → Right → Left 依序找第一个
//! 放得下的;全都放不下取可用空间最大的方向并截断尺寸;交叉轴(与主方向正交)按
//! [`MenuAlign`] 在锚点跨度内对齐,越界时向屏幕内 clamp。

use mineral_config::MenuAlign;
use ratatui::layout::Rect;

/// 弹出方向偏好(相对锚点矩形)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// 锚点下方。
    Below,

    /// 锚点上方。
    Above,

    /// 锚点右侧。
    Right,

    /// 锚点左侧。
    Left,
}

impl Placement {
    /// 对侧方向(翻转用)。
    fn opposite(self) -> Self {
        match self {
            Self::Below => Self::Above,
            Self::Above => Self::Below,
            Self::Right => Self::Left,
            Self::Left => Self::Right,
        }
    }

    /// 该方向上以 `anchor` 为界的可用空间(宽, 高)。
    fn avail(self, anchor: Rect, screen: Rect) -> (u16, u16) {
        let right_edge = screen.x.saturating_add(screen.width);
        let bottom_edge = screen.y.saturating_add(screen.height);
        match self {
            Self::Below => (
                screen.width,
                bottom_edge.saturating_sub(anchor.y.saturating_add(anchor.height)),
            ),
            Self::Above => (screen.width, anchor.y.saturating_sub(screen.y)),
            Self::Right => (
                right_edge.saturating_sub(anchor.x.saturating_add(anchor.width)),
                screen.height,
            ),
            Self::Left => (anchor.x.saturating_sub(screen.x), screen.height),
        }
    }
}

/// 计算弹出菜单的最终矩形。
///
/// # Params:
///   - `anchor`: 锚点矩形(选中行 / 光标格)
///   - `placement`: 首选方向
///   - `align`: 交叉轴(与主方向正交)在锚点跨度内的对齐
///   - `want_w` / `want_h`: 期望尺寸
///   - `screen`: 可用屏幕区域
///
/// # Return:
///   最终绘制矩形;恒在 `screen` 内,尺寸不超过期望(屏幕装不下时截断)。
pub(crate) fn place(
    anchor: Rect,
    placement: Placement,
    align: MenuAlign,
    want_w: u16,
    want_h: u16,
    screen: Rect,
) -> Rect {
    let order = [
        placement,
        placement.opposite(),
        Placement::Below,
        Placement::Above,
        Placement::Right,
        Placement::Left,
    ];
    let fits = |p: Placement| {
        let (aw, ah) = p.avail(anchor, screen);
        aw >= want_w && ah >= want_h
    };
    let chosen = order.into_iter().find(|p| fits(*p)).unwrap_or_else(|| {
        // 全方向都放不下:取可用面积最大的方向,后续按其空间截断
        order
            .into_iter()
            .max_by_key(|p| {
                let (w, h) = p.avail(anchor, screen);
                u32::from(w.min(want_w)) * u32::from(h.min(want_h))
            })
            .unwrap_or(placement)
    });

    let (aw, ah) = chosen.avail(anchor, screen);
    let w = want_w.min(aw).min(screen.width);
    let h = want_h.min(ah).min(screen.height);

    let (x, y) = match chosen {
        Placement::Below => (
            cross_align(anchor.x, anchor.width, w, align),
            anchor.y.saturating_add(anchor.height),
        ),
        Placement::Above => (
            cross_align(anchor.x, anchor.width, w, align),
            anchor.y.saturating_sub(h),
        ),
        Placement::Right => (
            anchor.x.saturating_add(anchor.width),
            cross_align(anchor.y, anchor.height, h, align),
        ),
        Placement::Left => (
            anchor.x.saturating_sub(w),
            cross_align(anchor.y, anchor.height, h, align),
        ),
    };
    // 交叉轴越界时向屏幕内收
    let max_x = screen.x.saturating_add(screen.width).saturating_sub(w);
    let max_y = screen.y.saturating_add(screen.height).saturating_sub(h);
    Rect {
        x: x.clamp(screen.x, max_x.max(screen.x)),
        y: y.clamp(screen.y, max_y.max(screen.y)),
        width: w,
        height: h,
    }
}

/// 交叉轴对齐:在锚点跨度 `[start, start+span)` 内为尺寸 `size` 定起点。
/// 起点 = `start + (span - size) × 比例`,比例取自 [`MenuAlign::permille`]
/// (0 贴起点 ~ 1000 贴终点),整数定点四舍五入。菜单比锚点宽时 `span - size` 为负、
/// 对称溢出(负坐标钳到 0,最终再由 `place` 的屏幕 clamp 收回)。
fn cross_align(start: u16, span: u16, size: u16, align: MenuAlign) -> u16 {
    let (start, span, size) = (i64::from(start), i64::from(span), i64::from(size));
    let off = (span - size) * i64::from(align.permille());
    // 四舍五入(numerator 可正可负,按符号补半再整除)。
    let rounded = (off + off.signum() * 500) / 1000;
    u16::try_from((start + rounded).max(0)).unwrap_or(0)
}
