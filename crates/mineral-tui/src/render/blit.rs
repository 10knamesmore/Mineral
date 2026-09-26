//! 离屏内容窗口搬运:进出场动画途中让真实内容跟着动,而不是只画纯色空壳。
//!
//! 内容先按**完全展开尺寸**渲染到离屏 [`Buffer`](布局只算一次,不随动画逐帧 reflow),
//! 再按动画进度把可见窗口整格搬进屏幕缓冲;亚格(1/8 cell)精度只能落在最前沿一格的
//! 八分块上 —— 文字没法画半个,内容窗口整格推进、前沿块平滑滑动,二者叠加成"内容
//! 跟着边缘进来"的运动感。浮层弹出与通知卡片的进出场共用这组基元。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::render::cells::{left_eighth, lower_eighth};
use crate::render::clear::Clear;

/// 水平滑入的贴边侧(= 完全展开时窗口锚定、动画期间保持不动的那条边)。
#[derive(Clone, Copy)]
pub(crate) enum HAnchor {
    /// 贴左缘,前沿向右推进(抽屉从左推入)。
    Left,

    /// 贴右缘,前沿向左推进(卡片自右滑入)。
    Right,
}

/// 前沿分数格的配色:`fill` 是面板体色(块的实心侧),`bg` 是屏幕底色(露出侧)。
#[derive(Clone, Copy)]
pub(crate) struct EdgeColors {
    /// 面板体色(前沿块实心部分)。
    pub(crate) fill: Color,

    /// 屏幕底色(前沿块未覆盖部分,反色补齐用)。
    pub(crate) bg: Color,
}

/// 把离屏 `src` 的整格窗口 `win`(src 坐标系,绝对)逐 cell 拷到 `dst`,窗口左上角
/// 落在 `(dst_x, dst_y)`。越出 `dst` 的部分静默丢弃。
///
/// 窗口右缘切到宽字符(CJK)前半时,该 cell 退化为同样式空格 —— 半个字符画不了,
/// 留着宽字符会溢出窗口、盖住前沿块。
///
/// # Params:
///   - `dst`: 目标(屏幕)缓冲
///   - `src`: 离屏源缓冲
///   - `win`: 源窗口(src 坐标系)
///   - `dst_x`: 窗口在目标上的左上角 x
///   - `dst_y`: 窗口在目标上的左上角 y
pub(crate) fn copy_window(dst: &mut Buffer, src: &Buffer, win: Rect, dst_x: u16, dst_y: u16) {
    for row in 0..win.height {
        let sy = win.y.saturating_add(row);
        let dy = dst_y.saturating_add(row);
        for col in 0..win.width {
            let sx = win.x.saturating_add(col);
            let Some(cell) = src.cell((sx, sy)) else {
                continue;
            };
            let mut cell = cell.clone();
            let sym_w = u16::try_from(UnicodeWidthStr::width(cell.symbol())).unwrap_or(1);
            if u32::from(col) + u32::from(sym_w) > u32::from(win.width) {
                cell.set_symbol(" ");
            }
            if let Some(slot) = dst.cell_mut((dst_x.saturating_add(col), dy)) {
                *slot = cell;
            }
        }
    }
}

/// 水平滑入/滑出:`anchor` 侧贴边,可见宽 `cur_w_e`(1/8 cell 单位),内容窗口随前沿
/// 整格平移(贴左 = 取离屏**右侧**列、贴右 = 取**左侧**列 —— 即前沿侧的边框最先进场),
/// 前沿分数格画八分块。退场走同一几何(进度反向),天然对称。
///
/// # Params:
///   - `buf`: 目标(屏幕)缓冲
///   - `off`: 离屏满尺寸内容(坐标系与 `full` 一致)
///   - `full`: 完全展开矩形
///   - `cur_w_e`: 当前可见宽,1/8 cell 单位(`0..=full.width*8`;不足一格不画)
///   - `anchor`: 贴边侧
///   - `edge`: 前沿分数格配色
pub(crate) fn slide_h(
    buf: &mut Buffer,
    off: &Buffer,
    full: Rect,
    cur_w_e: u32,
    anchor: HAnchor,
    edge: EdgeColors,
) {
    if full.width == 0 || full.height == 0 || cur_w_e < 8 {
        return;
    }
    let whole = u16::try_from(cur_w_e / 8)
        .unwrap_or(full.width)
        .min(full.width);
    let frac = cur_w_e % 8;
    let has_edge = frac > 0 && whole < full.width;
    let span = whole.saturating_add(u16::from(has_edge));

    // 先 Clear 可见包围盒(整格窗口 + 前沿格),防底层 UI 从边缘透出。
    let x0 = match anchor {
        HAnchor::Left => full.x,
        HAnchor::Right => full.right().saturating_sub(span),
    };
    Clear.render(Rect::new(x0, full.y, span, full.height), buf);

    match anchor {
        HAnchor::Left => {
            let win = Rect::new(
                full.right().saturating_sub(whole),
                full.y,
                whole,
                full.height,
            );
            copy_window(buf, off, win, full.x, full.y);
            if has_edge {
                let style = Style::new().fg(edge.fill);
                paint_col(
                    buf,
                    full.x.saturating_add(whole),
                    full,
                    left_eighth(frac),
                    style,
                );
            }
        }
        HAnchor::Right => {
            let dst_x = full.right().saturating_sub(whole);
            let win = Rect::new(full.x, full.y, whole, full.height);
            copy_window(buf, off, win, dst_x, full.y);
            if has_edge {
                // 反色右对齐:cell 右部 frac/8 实心、左部露底。
                let style = Style::new().fg(edge.bg).bg(edge.fill);
                paint_col(
                    buf,
                    dst_x.saturating_sub(1),
                    full,
                    left_eighth(8 - frac),
                    style,
                );
            }
        }
    }
}

/// 顶边锚定的垂直揭开:内容定格终位,可见高 `cur_h_e`(1/8 cell 单位)自顶向下逐行
/// 露出(reveal),底缘分数格画反色下八分块(cell 上部实心、下部露底)。
///
/// # Params:
///   - `buf`: 目标(屏幕)缓冲
///   - `off`: 离屏满尺寸内容(坐标系与 `full` 一致)
///   - `full`: 完全展开矩形
///   - `cur_h_e`: 当前可见高,1/8 cell 单位(不足一格不画)
///   - `edge`: 底缘分数格配色
pub(crate) fn reveal_v_top(
    buf: &mut Buffer,
    off: &Buffer,
    full: Rect,
    cur_h_e: u32,
    edge: EdgeColors,
) {
    if full.width == 0 || full.height == 0 || cur_h_e < 8 {
        return;
    }
    let whole = u16::try_from(cur_h_e / 8)
        .unwrap_or(full.height)
        .min(full.height);
    let frac = cur_h_e % 8;
    let has_edge = frac > 0 && whole < full.height;
    let rows = whole.saturating_add(u16::from(has_edge));

    Clear.render(Rect::new(full.x, full.y, full.width, rows), buf);
    copy_window(
        buf,
        off,
        Rect::new(full.x, full.y, full.width, whole),
        full.x,
        full.y,
    );
    if has_edge {
        let y = full.y.saturating_add(whole);
        let style = Style::new().fg(edge.bg).bg(edge.fill);
        let glyph = lower_eighth(8 - frac);
        for x in full.x..full.right() {
            buf.set_string(x, y, glyph, style);
        }
    }
}

/// 在 `full` 高度范围内把第 `x` 列整列画成 `glyph`(前沿分数格专用)。
fn paint_col(buf: &mut Buffer, x: u16, full: Rect, glyph: &str, style: Style) {
    for y in full.y..full.bottom() {
        buf.set_string(x, y, glyph, style);
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    use super::copy_window;

    /// 取 `(x, y)` 的符号,缺格报错(不用索引/unwrap)。
    fn sym(buf: &Buffer, x: u16, y: u16) -> color_eyre::Result<String> {
        Ok(buf
            .cell((x, y))
            .ok_or_else(|| eyre!("cell ({x},{y}) 缺失"))?
            .symbol()
            .to_owned())
    }

    /// 整格窗口平移搬运:源窗口内容落到目标平移后的位置,窗口外的目标格不被碰。
    #[test]
    fn copy_window_translates_cells() -> color_eyre::Result<()> {
        let mut src = Buffer::empty(Rect::new(0, 0, 10, 3));
        src.set_string(0, 1, "abcdef", Style::new());
        let mut dst = Buffer::empty(Rect::new(0, 0, 10, 4));
        dst.set_string(0, 3, "zz", Style::new());

        // 源窗口 = (1,1) 起 3x1("bcd"),搬到 (5,2)。
        copy_window(
            &mut dst,
            &src,
            Rect::new(1, 1, 3, 1),
            /*dst_x*/ 5,
            /*dst_y*/ 2,
        );
        assert_eq!(sym(&dst, 5, 2)?, "b");
        assert_eq!(sym(&dst, 6, 2)?, "c");
        assert_eq!(sym(&dst, 7, 2)?, "d");
        assert_eq!(sym(&dst, 8, 2)?, " ", "窗口右侧之外不该有内容");
        assert_eq!(sym(&dst, 0, 3)?, "z", "窗口外的目标格保持原样");
        Ok(())
    }

    /// 窗口右缘切到 CJK 宽字符前半:该 cell 退化为空格,不溢出窗口。
    #[test]
    fn copy_window_blanks_cut_wide_char() -> color_eyre::Result<()> {
        let mut src = Buffer::empty(Rect::new(0, 0, 8, 1));
        src.set_string(0, 0, "中文", Style::new());
        let mut dst = Buffer::empty(Rect::new(0, 0, 8, 1));

        // 窗口宽 3:"中"(0..2)完整、"文"(2..4)被切半 → 空格。
        copy_window(&mut dst, &src, Rect::new(0, 0, 3, 1), 0, 0);
        assert_eq!(sym(&dst, 0, 0)?, "中", "完整宽字符照搬");
        assert_eq!(sym(&dst, 2, 0)?, " ", "被切半的宽字符退化为空格");
        Ok(())
    }

    /// 目标越界的格静默丢弃,不 panic。
    #[test]
    fn copy_window_drops_out_of_bounds() -> color_eyre::Result<()> {
        let mut src = Buffer::empty(Rect::new(0, 0, 4, 1));
        src.set_string(0, 0, "abcd", Style::new());
        let mut dst = Buffer::empty(Rect::new(0, 0, 4, 1));
        // dst_x=2:c/d 落在 dst 外。
        copy_window(&mut dst, &src, Rect::new(0, 0, 4, 1), 2, 0);
        assert_eq!(sym(&dst, 2, 0)?, "a");
        assert_eq!(sym(&dst, 3, 0)?, "b");
        Ok(())
    }
}
