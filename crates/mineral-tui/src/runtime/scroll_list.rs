//! 可滚动列表的完整 UI-local 态:光标(选中行)+ 视口滚动(nvim 手感 + 缓动平移)二者绑定。
//!
//! 把 [`ListCursor`] 与 [`ListScroll`] 收成一个部件,杜绝「有光标无滚动」——任何列表面持一个
//! `ScrollList` 即同时拿到选中与滚动,渲染统一经 [`Self::offset`] 出 offset 再喂
//! `TableState::offset`(渲染层 helper 见 components),不会再出现某处用裸 `TableState`
//! (offset 复位 0)让 ratatui 每帧重求最小可见滚动、把选中行钉死视口底边。
//!
//! items 不在此持有(队列 / 歌单 / 搜索结果是后端或派生态,住在别处),故移动 / 钳制 / 渲染
//! 都按列表长度 `len` 与视口 `viewport` 参数化。光标移动([`Self::move_by`])走按键路径
//! (`&mut`)、视口推进([`Self::offset`] 的 `Advancing`)走渲染路径(`&`,内部 `RefCell`,
//! 每帧恰一次)——二者分处不同帧路径。

use crate::runtime::action::SelectionMove;
use crate::runtime::list_cursor::ListCursor;
use crate::runtime::scroll::ListScroll;

/// 渲染时视口的推进语义(喂给 [`ScrollList::offset`])。
#[derive(Clone, Copy)]
pub(crate) enum ScrollMotion {
    /// 稳态实拍:推进缓动动画一拍,按光标重算滚动目标(渲染端每帧恰调一次)。
    Advancing {
        /// 光标与视口上下边缘的最小行距(配置 `behavior.scrolloff`)。
        scrolloff: usize,

        /// 平移缓动拍数。
        glide_ticks: u16,
    },

    /// 瞬态几何(离屏合成 / 全屏 morph):只读展示当前位置,不推进动画、不改滚动目标。
    Frozen,
}

/// 一个可滚动列表的 UI-local 态:光标 + 视口滚动。
#[derive(Clone)]
pub(crate) struct ScrollList {
    /// 选中行下标(UI-local;移动 / 钳制走 [`ListCursor`])。
    cursor: ListCursor,

    /// 视口滚动态(跨帧持久 offset + 缓动平移,走 [`ListScroll`])。
    scroll: ListScroll,
}

impl ScrollList {
    /// 新建:光标在首行、视口停顶、无平移。
    pub(crate) fn new() -> Self {
        Self {
            cursor: ListCursor::new(0),
            scroll: ListScroll::new(),
        }
    }

    /// 新建并把光标 + 视口直接落在 `sel`(打开浮层定位在播歌等;视口瞬时落位,不从队首长程滑)。
    pub(crate) fn at(sel: usize) -> Self {
        let mut me = Self::new();
        me.place(sel, 0);
        me
    }

    /// 当前选中行下标。
    pub(crate) fn sel(&self) -> usize {
        self.cursor.sel()
    }

    /// 仅设光标下标(视口不动,留给下一帧渲染按 scrolloff 缓动跟随)。
    pub(crate) fn set_sel(&mut self, sel: usize) {
        self.cursor.set(sel);
    }

    /// 按一次移动指令移动光标,钳在 `[0, len-1]`(视口由渲染端跟随)。
    pub(crate) fn move_by(&mut self, mv: SelectionMove, len: usize) {
        self.cursor.move_by(mv, len);
    }

    /// `<C-d>` 族:视口目标与光标同移 `delta` 行(vim 语义,保持光标屏上相对位置)。
    /// 下界钳 0、上界由渲染端钳;光标按 `len` 钳首末。
    ///
    /// # Params:
    ///   - `delta`: 行数增量(向下为正)
    ///   - `len`: 当前列表长度
    ///   - `glide_ticks`: 平移缓动拍数
    pub(crate) fn page(&mut self, delta: i64, len: usize, glide_ticks: u16) {
        self.scroll.nudge(delta, glide_ticks);
        let rows = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
        let mv = if delta >= 0 {
            SelectionMove::Down(rows)
        } else {
            SelectionMove::Up(rows)
        };
        self.cursor.move_by(mv, len);
    }

    /// 列表变短后把光标钳回 `[0, len-1]`(过滤 / 异步刷新后防越界);空列表归 0。
    pub(crate) fn clamp(&mut self, len: usize) {
        self.cursor.clamp(len);
    }

    /// 光标落 `sel`、视口瞬时落位使该行距视口顶约 `anchor` 行(无缓动)。
    /// 视图重置 / 进列表 / 搜索复位等「不该有滚动感」的场合用;越界修正由渲染端首帧瞬时落。
    ///
    /// # Params:
    ///   - `sel`: 目标光标行
    ///   - `anchor`: 该行距视口顶的目标行距(`0` = 落顶,渲染端再按 scrolloff 钳)
    pub(crate) fn place(&mut self, sel: usize, anchor: usize) {
        self.cursor.set(sel);
        self.scroll.snap_to(sel.saturating_sub(anchor));
    }

    /// 当前滚动目标(视口首行)。位置记忆记录「光标屏上相对行」时读取。
    pub(crate) fn scroll_target(&self) -> usize {
        self.scroll.target_rows()
    }

    /// 本帧视口首行 offset(喂 `TableState::offset`);高亮行另经 `pin_cursor` 钳边。
    ///
    /// # Params:
    ///   - `len`: 列表总行数
    ///   - `viewport`: 视口行数
    ///   - `motion`: 推进(稳态实拍)/ 冻结(瞬态几何)
    ///
    /// # Return:
    ///   本帧视口首行(恒在 `[0, len-viewport]`)。
    pub(crate) fn offset(&self, len: usize, viewport: usize, motion: ScrollMotion) -> usize {
        match motion {
            ScrollMotion::Advancing {
                scrolloff,
                glide_ticks,
            } => {
                self.scroll
                    .render_offset(self.cursor.sel(), len, viewport, scrolloff, glide_ticks)
            }
            ScrollMotion::Frozen => self.scroll.frozen_offset(len, viewport),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScrollList, ScrollMotion};
    use crate::runtime::action::SelectionMove;

    /// 推进档(`Advancing`)收敛后,深处光标的视口保留 scrolloff:不贴底。
    /// 这是 [`ScrollList`] 存在的根本理由(裸 `TableState` 会把选中行钉视口底边)。
    #[test]
    fn advancing_keeps_scrolloff_below_deep_cursor() {
        let mut list = ScrollList::new();
        list.set_sel(25);
        let adv = ScrollMotion::Advancing {
            scrolloff: 3,
            glide_ticks: 4,
        };
        // 多帧缓动收敛。
        let mut off = 0;
        for _ in 0..8 {
            off = list.offset(/*len*/ 30, /*viewport*/ 10, adv);
        }
        // sel=25 在视口内,且下方仍留 ≥ scrolloff 行(25 - off <= viewport-1-so)。
        assert!(off <= 25 && 25 < off + 10, "选中行可见: off={off}");
        assert!(
            25 - off <= 10 - 1 - 3,
            "选中行下方应留 ≥ scrolloff: off={off}"
        );
    }

    /// `move_by` 移光标、`clamp` 列表变短后钳回末行。
    #[test]
    fn move_and_clamp_track_len() {
        let mut list = ScrollList::new();
        list.move_by(SelectionMove::Down(5), /*len*/ 30);
        assert_eq!(list.sel(), 5);
        list.move_by(SelectionMove::Last, 30);
        assert_eq!(list.sel(), 29, "Last 跳末行");
        list.clamp(/*len*/ 10);
        assert_eq!(list.sel(), 9, "列表缩短钳回末行");
    }

    /// `page`:视口与光标同移 n 行(向下 / 向上);光标按 len 钳首末。
    #[test]
    fn page_moves_cursor_with_viewport() {
        let mut list = ScrollList::new();
        list.page(/*delta*/ 5, /*len*/ 30, /*glide_ticks*/ 2);
        assert_eq!(list.sel(), 5, "下翻 5 行光标同移");
        list.page(-100, 30, 2);
        assert_eq!(list.sel(), 0, "上翻越界钳首行");
    }

    /// `place`:光标落 sel、视口瞬时落位(`Frozen` 读当前位置不推进);首帧即到位无缓动。
    #[test]
    fn place_snaps_viewport_without_glide() {
        let mut list = ScrollList::new();
        list.place(/*sel*/ 20, /*anchor*/ 3);
        // Frozen 读当前位置:snap 到 sel-anchor=17,钳进边界。
        let off = list.offset(/*len*/ 30, /*viewport*/ 10, ScrollMotion::Frozen);
        assert_eq!(off, 17, "place 后视口瞬时落在 sel-anchor");
        assert_eq!(list.sel(), 20, "光标落 sel");
    }

    /// `at`:构造即定位(光标 + 视口都落 sel 附近)。
    #[test]
    fn at_positions_on_construct() {
        let list = ScrollList::at(15);
        assert_eq!(list.sel(), 15);
        let off = list.offset(30, 10, ScrollMotion::Frozen);
        assert_eq!(off, 15, "视口瞬时落在 sel(anchor=0)");
    }

    /// `Frozen` 不推进动画:同一 `ScrollList` 连调多次 `Frozen` offset 幂等(离屏合成多次渲染同帧安全)。
    #[test]
    fn frozen_offset_is_idempotent() {
        let mut list = ScrollList::new();
        list.place(20, 3);
        let a = list.offset(30, 10, ScrollMotion::Frozen);
        let b = list.offset(30, 10, ScrollMotion::Frozen);
        let c = list.offset(30, 10, ScrollMotion::Frozen);
        assert_eq!((a, b), (b, c), "Frozen 多次调用同值(不推进、不改目标)");
    }
}
