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

#[cfg(test)]
mod tests {
    use mineral_config::MenuAlign;
    use proptest::prelude::*;
    use ratatui::layout::Rect;

    use super::{Placement, place};

    /// 任意方向。
    fn arb_placement() -> impl Strategy<Value = Placement> {
        proptest::sample::select(vec![
            Placement::Below,
            Placement::Above,
            Placement::Right,
            Placement::Left,
        ])
    }

    /// 任意对齐。
    fn arb_align() -> impl Strategy<Value = MenuAlign> {
        proptest::sample::select(vec![MenuAlign::Left, MenuAlign::Center, MenuAlign::Right])
    }

    /// 屏内随机锚点(1x1 至屏宽高)。
    fn arb_anchor(sw: u16, sh: u16) -> impl Strategy<Value = Rect> {
        (0..sw, 0..sh, 1..=4_u16, 1..=2_u16).prop_map(move |(x, y, w, h)| Rect {
            x,
            y,
            width: w.min(sw - x).max(1),
            height: h.min(sh - y).max(1),
        })
    }

    proptest! {
        /// 不变量:结果恒在屏幕内、尺寸不超过期望。
        #[test]
        fn result_stays_on_screen(
            (sw, sh) in (8..=200_u16, 4..=60_u16),
            placement in arb_placement(),
            align in arb_align(),
            want_w in 1..=80_u16,
            want_h in 1..=30_u16,
        ) {
            let screen = Rect { x: 0, y: 0, width: sw, height: sh };
            let anchor_strategy = arb_anchor(sw, sh);
            proptest!(|(anchor in anchor_strategy)| {
                let got = place(anchor, placement, align, want_w, want_h, screen);
                prop_assert!(got.x.saturating_add(got.width) <= sw, "右越界: {got:?}");
                prop_assert!(got.y.saturating_add(got.height) <= sh, "下越界: {got:?}");
                prop_assert!(got.width <= want_w && got.height <= want_h);
                prop_assert!(got.width >= 1 && got.height >= 1 || want_w == 0 || want_h == 0);
            });
        }
    }

    /// 空间足够时贴首选侧:Below 在锚点正下方。
    #[test]
    fn prefers_requested_side_when_fits() {
        let screen = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let anchor = Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 1,
        };
        let got = place(anchor, Placement::Below, MenuAlign::Left, 15, 6, screen);
        assert_eq!((got.x, got.y), (10, 6));
        assert_eq!((got.width, got.height), (15, 6));
    }

    /// 交叉轴对齐:窄菜单(宽 8)在宽锚点(x=10, 宽 20)下方,
    /// Left 贴左缘、Center 居中、Right 贴右缘。
    #[test]
    fn aligns_cross_axis_within_anchor() {
        let screen = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let anchor = Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 1,
        };
        let x = |a| place(anchor, Placement::Below, a, 8, 6, screen).x;
        assert_eq!(x(MenuAlign::Left), 10, "贴锚点左缘");
        assert_eq!(x(MenuAlign::Center), 16, "居中:10 + (20-8)/2");
        assert_eq!(x(MenuAlign::Right), 22, "贴锚点右缘:10 + 20 - 8");
        // 精确比例 0.25:10 + round((20-8) × 0.25) = 10 + 3 = 13。
        assert_eq!(x(MenuAlign::Fraction(0.25)), 13, "比例 0.25");
        assert_eq!(x(MenuAlign::Fraction(0.0)), 10, "比例 0 = 贴左");
        assert_eq!(x(MenuAlign::Fraction(1.0)), 22, "比例 1 = 贴右");
    }

    /// 首选放不下翻转对侧:贴底锚点 Below 不够 → Above。
    #[test]
    fn flips_to_opposite_when_preferred_lacks_space() {
        let screen = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let anchor = Rect {
            x: 10,
            y: 22,
            width: 20,
            height: 1,
        };
        let got = place(anchor, Placement::Below, MenuAlign::Left, 15, 6, screen);
        // Above:y = 22 - 6 = 16
        assert_eq!((got.x, got.y), (10, 16));
    }

    /// 屏幕整体装不下期望尺寸:截断而非 panic/越界。
    #[test]
    fn truncates_when_screen_too_small() {
        let screen = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        let anchor = Rect {
            x: 2,
            y: 1,
            width: 3,
            height: 1,
        };
        let got = place(anchor, Placement::Right, MenuAlign::Left, 40, 10, screen);
        assert!(got.x.saturating_add(got.width) <= 10);
        assert!(got.y.saturating_add(got.height) <= 4);
    }
}
