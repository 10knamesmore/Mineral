//! 下钻 / 返回与 artist 双区切换共用的横向 sweep 列合成原语：计算每列的取样帧与源列号，
//! 并把列写入目标缓冲，保留透明背景。

use mineral_config::SweepStyle;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// 缓动进度满值（千分比）。
pub(super) const FULL: u32 = 1000;

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
/// 返回 sweep 与 artist 双区切换共用此映射，相同风格与方向得到相同列运动。
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
        // SweepStyle 是 #[non_exhaustive];本接线不识别的变体按 Push 处理。
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
