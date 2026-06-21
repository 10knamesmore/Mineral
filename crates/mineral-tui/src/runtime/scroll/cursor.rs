//! 列表光标(UI-local 选中下标)的纯逻辑:按 [`SelectionMove`] 移动、按 `len` 钳制。
//!
//! 与视口滚动配对——光标管「选中第几行」,滚动管「视口画哪一段」。items 不在此持有
//! (队列 / 歌单等是后端或派生态,住在别处),故移动 / 钳制按列表长度 `len` 参数化。
//!
//! [`SelectionMove`]: crate::runtime::action::SelectionMove

use crate::runtime::action::SelectionMove;

/// 列表光标:只持选中行下标(UI-local),移动 / 钳制按列表长度 `len` 参数化。
///
/// items 不在此持有——队列 / 歌单等是后端或派生态,住在别处;本类型只管「选中第几行」
/// 这个纯客户端态,后端同步逻辑碰都不该碰它(仅在列表变短时 [`Self::clamp`] 防越界)。
#[derive(Clone)]
pub(crate) struct ListCursor {
    /// 选中行下标。
    sel: usize,
}

impl ListCursor {
    /// 新建,光标落 `sel`(调用方保证落在初始列表内,或随后 [`Self::clamp`])。
    pub(crate) fn new(sel: usize) -> Self {
        Self { sel }
    }

    /// 按一次移动指令移动光标,钳在 `[0, len-1]`;空列表(`len == 0`)恒落 0。
    ///
    /// # Params:
    ///   - `mv`: 移动指令(j/k/J/K/g/G 归一)
    ///   - `len`: 当前列表长度(items 住在别处,故按 len 参数化)
    pub(crate) fn move_by(&mut self, mv: SelectionMove, len: usize) {
        let max = len.saturating_sub(1);
        self.sel = match mv {
            SelectionMove::Down(n) => self.sel.saturating_add(n).min(max),
            SelectionMove::Up(n) => self.sel.saturating_sub(n),
            SelectionMove::First => 0,
            SelectionMove::Last => max,
        };
    }

    /// 列表变短后把光标夹回 `[0, len-1]`(过滤 / 异步刷新后防越界);空列表归 0。
    pub(crate) fn clamp(&mut self, len: usize) {
        self.sel = self.sel.min(len.saturating_sub(1));
    }

    /// 直接落到某下标(调用方保证有效,或随后 [`Self::clamp`]);视口不在此管。
    pub(crate) fn set(&mut self, sel: usize) {
        self.sel = sel;
    }

    /// 当前选中行下标。
    pub(crate) fn sel(&self) -> usize {
        self.sel
    }
}

#[cfg(test)]
mod tests {
    use super::ListCursor;
    use crate::runtime::action::SelectionMove;

    /// 下移按 `n` 推进,越界钳到末行(`len-1`)。
    #[test]
    fn move_down_clamps_to_last() {
        let mut c = ListCursor::new(0);
        c.move_by(SelectionMove::Down(3), /*len*/ 5);
        assert_eq!(c.sel(), 3, "下移 3 行");
        c.move_by(SelectionMove::Down(10), /*len*/ 5);
        assert_eq!(c.sel(), 4, "下移越界钳到末行");
    }

    /// 上移按 `n` 回退,越界钳到首行(0)。
    #[test]
    fn move_up_saturates_at_first() {
        let mut c = ListCursor::new(4);
        c.move_by(SelectionMove::Up(2), /*len*/ 5);
        assert_eq!(c.sel(), 2, "上移 2 行");
        c.move_by(SelectionMove::Up(10), /*len*/ 5);
        assert_eq!(c.sel(), 0, "上移越界钳到首行");
    }

    /// First 跳首行、Last 跳末行。
    #[test]
    fn move_first_and_last() {
        let mut c = ListCursor::new(2);
        c.move_by(SelectionMove::Last, /*len*/ 7);
        assert_eq!(c.sel(), 6, "Last 跳末行");
        c.move_by(SelectionMove::First, /*len*/ 7);
        assert_eq!(c.sel(), 0, "First 跳首行");
    }

    /// 空列表(`len == 0`)任意移动都落 0、不溢出(`len-1` 不得 usize 回绕)。
    #[test]
    fn move_on_empty_list_stays_zero() {
        let mut c = ListCursor::new(0);
        c.move_by(SelectionMove::Down(5), /*len*/ 0);
        assert_eq!(c.sel(), 0, "空列表下移仍 0");
        c.move_by(SelectionMove::Last, /*len*/ 0);
        assert_eq!(c.sel(), 0, "空列表 Last 仍 0");
    }

    /// `clamp` 把越界光标夹回 `len-1`;空列表归 0。
    #[test]
    fn clamp_caps_sel() {
        let mut c = ListCursor::new(9);
        c.clamp(/*len*/ 3);
        assert_eq!(c.sel(), 2, "钳到末行");
        c.clamp(/*len*/ 0);
        assert_eq!(c.sel(), 0, "空列表归 0");
    }
}
