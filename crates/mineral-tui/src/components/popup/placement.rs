//! PopMenu 的锚定放置算法:纯函数,给定锚点矩形、方向偏好、期望尺寸与屏幕,
//! 算出最终绘制矩形。
//!
//! 规则:首选方向放不下 → 对侧 → Below → Above → Right → Left 依序找第一个
//! 放得下的;全都放不下取可用空间最大的方向并截断尺寸;横轴与锚点起点对齐,
//! 越界时向屏幕内 clamp。

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
///   - `want_w` / `want_h`: 期望尺寸
///   - `screen`: 可用屏幕区域
///
/// # Return:
///   最终绘制矩形;恒在 `screen` 内,尺寸不超过期望(屏幕装不下时截断)。
pub(crate) fn place(
    anchor: Rect,
    placement: Placement,
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
        Placement::Below => (anchor.x, anchor.y.saturating_add(anchor.height)),
        Placement::Above => (anchor.x, anchor.y.saturating_sub(h)),
        Placement::Right => (anchor.x.saturating_add(anchor.width), anchor.y),
        Placement::Left => (anchor.x.saturating_sub(w), anchor.y),
    };
    // 横轴(与主方向正交的轴)越界时向屏幕内收
    let max_x = screen.x.saturating_add(screen.width).saturating_sub(w);
    let max_y = screen.y.saturating_add(screen.height).saturating_sub(h);
    Rect {
        x: x.clamp(screen.x, max_x.max(screen.x)),
        y: y.clamp(screen.y, max_y.max(screen.y)),
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
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
            want_w in 1..=80_u16,
            want_h in 1..=30_u16,
        ) {
            let screen = Rect { x: 0, y: 0, width: sw, height: sh };
            let anchor_strategy = arb_anchor(sw, sh);
            proptest!(|(anchor in anchor_strategy)| {
                let got = place(anchor, placement, want_w, want_h, screen);
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
        let got = place(anchor, Placement::Below, 15, 6, screen);
        assert_eq!((got.x, got.y), (10, 6));
        assert_eq!((got.width, got.height), (15, 6));
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
        let got = place(anchor, Placement::Below, 15, 6, screen);
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
        let got = place(anchor, Placement::Right, 40, 10, screen);
        assert!(got.x.saturating_add(got.width) <= 10);
        assert!(got.y.saturating_add(got.height) <= 4);
    }
}
