//! 下钻 / 返回与 artist 双区切换共用的横向 sweep 列合成原语：纯几何（某屏幕列取样自哪一帧的
//! 哪一列）+ 列搬运。上层（出发 / 目标帧各自离屏渲染、按风格与方向驱动）在父模块，这里只
//! 管「第 c 列该取谁的第几列」与「把一列搬过去」。

use mineral_config::SweepStyle;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// 横向 sweep 合成时，某屏幕列取样自哪一帧。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SweepLayer {
    /// 出发帧（滑出 / 原地）。
    From,

    /// 目标帧（滑入 / 覆盖）。
    To,
}

/// sweep 合成的纯几何：给定风格 / 方向 / 当前列 `c` / 区宽 `w` / 目标已进列数 `advance`，
/// 定出该列取出发帧还是目标帧、及其相对列号。`is_push`（下钻）目标从右来、否则（返回）从
/// 左来；[`SweepStyle::Cover`] 出发帧原地不动、[`SweepStyle::Push`] 出发帧整体让位。下钻 /
/// 返回 sweep 与 artist 双区切换共用此一处映射——动法不再各写一套、不会再漂移。
///
/// # Params:
///   - `style`: 过渡风格（配置 `view_sweep`）
///   - `is_push`: 方向，`true` = 下钻（目标右入）、`false` = 返回（目标左入）
///   - `c`: 当前屏幕相对列（`0..w`）
///   - `w`: 合成区宽
///   - `advance`: 目标帧已进入的列数（`0..=w`，由缓动进度折算）
///
/// # Return:
///   `(取样帧, 该帧相对列号)`。
pub(super) fn sweep_column(
    style: SweepStyle,
    is_push: bool,
    c: u16,
    w: u16,
    advance: u16,
) -> (SweepLayer, u16) {
    match style {
        SweepStyle::Cover => {
            if is_push {
                // 目标从右覆盖：右 advance 列取目标帧，其余出发帧原地。
                let split = w.saturating_sub(advance);
                if c < split {
                    (SweepLayer::From, c)
                } else {
                    (SweepLayer::To, c - split)
                }
            } else if c < advance {
                // 目标从左覆盖：左 advance 列取目标帧。
                (SweepLayer::To, c)
            } else {
                (SweepLayer::From, c)
            }
        }
        // 非穷尽（`#[non_exhaustive]`）→ 未接线的新风格按 Push 兜底。
        SweepStyle::Push | _ => {
            if is_push {
                // 出发帧整体左移 advance、目标从右补入。
                if c + advance < w {
                    (SweepLayer::From, c + advance)
                } else {
                    (SweepLayer::To, c + advance - w)
                }
            } else if c < advance {
                // 镜像：目标从左补入（右缘落在第 advance 列），出发帧整体右移。
                (SweepLayer::To, c + (w - advance))
            } else {
                (SweepLayer::From, c - advance)
            }
        }
    }
}

/// 把离屏 `src` 的第 `src_c` 列（相对 `area`）整列搬到 `dst` 的第 `dst_c` 列。
pub(super) fn copy_col(dst: &mut Buffer, area: Rect, src: &Buffer, dst_c: u16, src_c: u16) {
    let dx = area.x.saturating_add(dst_c);
    let sx = area.x.saturating_add(src_c);
    for ry in area.y..area.y.saturating_add(area.height) {
        if let Some(cell) = src.cell((sx, ry)) {
            let mut cell = cell.clone();
            // 离屏帧空 cell 的 Reset 底视作透明:回落到目标已铺的 backdrop 底(paint_backdrop
            // 铺的 theme.background + 氛围场)。否则整格搬回会把 backdrop 盖成终端默认底,下钻/
            // 返回过场腾出的列露出终端底洞——稳态面板背景全靠底层 backdrop,离屏合成必须让它透出。
            if matches!(cell.bg, Color::Reset)
                && let Some(under) = dst.cell((dx, ry))
            {
                cell.set_bg(under.bg);
            }
            if let Some(slot) = dst.cell_mut((dx, ry)) {
                *slot = cell;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_config::SweepStyle;

    use super::{SweepLayer, sweep_column};

    /// Cover 风格：出发帧原地不动，目标帧从一侧覆盖（下钻从右、返回从左）。
    /// 这是「下钻过渡与 compose_sweep / 左栏 view-sweep 共用同一列映射」的几何契约。
    #[test]
    fn sweep_column_cover_keeps_from_stationary() {
        let (w, advance) = (10u16, 4u16); // split = w - advance = 6
        // 下钻：左 split 列取出发帧（原地），右 advance 列取目标帧。
        assert_eq!(
            sweep_column(SweepStyle::Cover, true, 0, w, advance),
            (SweepLayer::From, 0)
        );
        assert_eq!(
            sweep_column(SweepStyle::Cover, true, 5, w, advance),
            (SweepLayer::From, 5)
        );
        assert_eq!(
            sweep_column(SweepStyle::Cover, true, 6, w, advance),
            (SweepLayer::To, 0)
        );
        assert_eq!(
            sweep_column(SweepStyle::Cover, true, 9, w, advance),
            (SweepLayer::To, 3)
        );
        // 返回：目标帧从左覆盖。
        assert_eq!(
            sweep_column(SweepStyle::Cover, false, 0, w, advance),
            (SweepLayer::To, 0)
        );
        assert_eq!(
            sweep_column(SweepStyle::Cover, false, 3, w, advance),
            (SweepLayer::To, 3)
        );
        assert_eq!(
            sweep_column(SweepStyle::Cover, false, 4, w, advance),
            (SweepLayer::From, 4)
        );
        assert_eq!(
            sweep_column(SweepStyle::Cover, false, 9, w, advance),
            (SweepLayer::From, 9)
        );
    }

    /// Push 风格：出发帧整体平移让位、目标帧从另一侧补入（与 Top Songs/Albums 切区、左栏
    /// 视图切换同款）。下钻出发帧左移（src_c = c+advance），返回出发帧右移（src_c = c-advance）。
    #[test]
    fn sweep_column_push_shifts_both() {
        let (w, advance) = (10u16, 4u16);
        // 下钻：出发帧左移、目标帧从右补入。
        assert_eq!(
            sweep_column(SweepStyle::Push, true, 0, w, advance),
            (SweepLayer::From, 4)
        );
        assert_eq!(
            sweep_column(SweepStyle::Push, true, 5, w, advance),
            (SweepLayer::From, 9)
        );
        assert_eq!(
            sweep_column(SweepStyle::Push, true, 6, w, advance),
            (SweepLayer::To, 0)
        );
        assert_eq!(
            sweep_column(SweepStyle::Push, true, 9, w, advance),
            (SweepLayer::To, 3)
        );
        // 返回：出发帧右移、目标帧从左补入（镜像）。
        assert_eq!(
            sweep_column(SweepStyle::Push, false, 0, w, advance),
            (SweepLayer::To, 6)
        );
        assert_eq!(
            sweep_column(SweepStyle::Push, false, 3, w, advance),
            (SweepLayer::To, 9)
        );
        assert_eq!(
            sweep_column(SweepStyle::Push, false, 4, w, advance),
            (SweepLayer::From, 0)
        );
        assert_eq!(
            sweep_column(SweepStyle::Push, false, 9, w, advance),
            (SweepLayer::From, 5)
        );
    }

    /// copy_col 把离屏空 cell 的 Reset 底视作透明：搬回目标时保留目标已铺的 backdrop 底，
    /// 只有源显式设的底才照搬。防回归：稳态面板背景全靠底层 paint_backdrop，离屏 Buffer::empty
    /// 的 Reset 底若被整格搬回会盖成终端默认底，下钻过场露洞。
    #[test]
    fn copy_col_treats_reset_bg_as_transparent() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::style::Color;

        let area = Rect::new(0, 0, 3, 2);
        let backdrop = Color::Rgb(1, 2, 3);
        let mut dst = Buffer::empty(area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(c) = dst.cell_mut((x, y)) {
                    c.set_bg(backdrop);
                }
            }
        }
        // 源：第 0 列全空（Reset 底）；第 1 列一格有内容 + 显式红底。
        let mut src = Buffer::empty(area);
        if let Some(c) = src.cell_mut((1, 0)) {
            c.set_symbol("x").set_bg(Color::Red);
        }
        super::copy_col(&mut dst, area, &src, /*dst_c*/ 0, /*src_c*/ 0);
        super::copy_col(&mut dst, area, &src, /*dst_c*/ 1, /*src_c*/ 1);

        assert_eq!(
            dst.cell((0, 0)).map(|c| c.bg),
            Some(backdrop),
            "空 cell 的 Reset 底回落到 backdrop"
        );
        assert_eq!(
            dst.cell((1, 0)).map(|c| c.bg),
            Some(Color::Red),
            "源显式设的底照搬，不被回落"
        );
    }

    /// 端点退化：advance=0 整屏取出发帧、advance=w 整屏取目标帧——两风格、两方向皆然。
    /// 保证 sweep 起末两帧与稳态单帧逐列一致，落定不跳。
    #[test]
    fn sweep_column_endpoints_are_identity() {
        let w = 8u16;
        for style in [SweepStyle::Cover, SweepStyle::Push] {
            for is_push in [true, false] {
                for c in 0..w {
                    assert_eq!(
                        sweep_column(style, is_push, c, w, /*advance*/ 0),
                        (SweepLayer::From, c),
                        "advance=0 应全取出发帧: style={style:?} push={is_push} c={c}"
                    );
                    assert_eq!(
                        sweep_column(style, is_push, c, w, /*advance*/ w),
                        (SweepLayer::To, c),
                        "advance=w 应全取目标帧: style={style:?} push={is_push} c={c}"
                    );
                }
            }
        }
    }
}
