//! 列表自己的光标、视口和 minimap 动画状态。
//!
//! 输入入口移动光标；[`ScrollList::prepare`] 根据布局调整目标，并按显式时钟推进动画。
//! 绘制通过 [`ScrollList::offset`] 和 [`ScrollList::position`] 只读采样。
//! 列表数据归调用方持有，本组件只需要列表长度与视口尺寸。

use crate::runtime::action::SelectionMove;
use crate::runtime::scroll::cursor::ListCursor;
use crate::runtime::scroll::position::{ListPosition, MagnetProgress};
use crate::runtime::scroll::viewport::ListScroll;

/// 准备阶段的视口更新策略。
#[derive(Clone, Copy)]
pub(crate) enum ScrollMotion {
    /// 稳定布局：按光标重算滚动目标；是否推进一拍由准备入口的 `advance` 决定。
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

    /// 全列表位置标记的缓动；与视口分开推进，视口未滚动时仍能跟随光标。
    position: ListPosition,

    /// 光标吸进在播标记后的颜色过渡；在准备阶段推进，视口无关。
    magnet: MagnetProgress,
}

impl ScrollList {
    /// 新建:光标在首行、视口停顶、无平移。
    pub(crate) fn new() -> Self {
        Self {
            cursor: ListCursor::new(0),
            scroll: ListScroll::new(),
            position: ListPosition::default(),
            magnet: MagnetProgress::default(),
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

    /// 仅设光标下标(视口不动,留给下一次准备按 scrolloff 缓动跟随)。
    pub(crate) fn set_sel(&mut self, sel: usize) {
        self.cursor.set(sel);
    }

    /// 按一次移动指令移动光标,钳在 `[0, len-1]`(视口由准备入口跟随)。
    pub(crate) fn move_by(&mut self, mv: SelectionMove, len: usize) {
        self.cursor.move_by(mv, len);
    }

    /// `<C-d>` 族:视口目标与光标同移 `delta` 行(vim 语义,保持光标屏上相对位置)。
    /// 下界钳 0、上界由准备入口钳;光标按 `len` 钳首末。
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
    /// 视图重置 / 进列表 / 搜索复位等「不该有滚动感」的场合用;越界修正由准备入口首帧瞬时落。
    ///
    /// # Params:
    ///   - `sel`: 目标光标行
    ///   - `anchor`: 该行距视口顶的目标行距(`0` = 落顶,准备入口再按 scrolloff 钳)
    pub(crate) fn place(&mut self, sel: usize, anchor: usize) {
        self.cursor.set(sel);
        self.scroll.snap_to(sel.saturating_sub(anchor));
        self.position.reset();
        self.magnet.reset();
    }

    /// 用稳定布局协调视口与位置动画；输入或 resize 可调目标，但不额外推进一拍。
    pub(crate) fn prepare(
        &mut self,
        len: usize,
        viewport: usize,
        motion: ScrollMotion,
        cursor_ticks: u16,
        advance: bool,
    ) {
        if let ScrollMotion::Advancing {
            scrolloff,
            glide_ticks,
        } = motion
        {
            self.scroll
                .prepare_offset(self.sel(), len, viewport, scrolloff, glide_ticks, advance);
            self.position
                .advance(self.sel(), len, cursor_ticks, advance);
        }
    }

    /// 准备阶段根据 minimap 的实际几何调整吸附动画。
    pub(crate) fn prepare_magnet(&mut self, absorbed: bool, ticks: u16, advance: bool) {
        self.magnet.advance(absorbed, ticks, advance);
    }

    /// 返回全列表位置标记的本帧坐标；空列表为 None，满值见 `position::POSITION_SCALE`。
    ///
    /// # Params:
    ///   - `len`: 当前显示列表长度。
    pub(crate) fn position(&self, len: usize) -> Option<u32> {
        self.position.frozen(self.sel(), len)
    }

    /// 光标吸进在播标记后的颜色过渡进度（[`MagnetProgress`]）。
    pub(crate) fn magnet(&self) -> &MagnetProgress {
        &self.magnet
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
    ///
    /// # Return:
    ///   本帧视口首行(恒在 `[0, len-viewport]`)。
    pub(crate) fn offset(&self, len: usize, viewport: usize) -> usize {
        self.scroll.frozen_offset(len, viewport)
    }
}

#[cfg(test)]
mod tests {
    use super::{ScrollList, ScrollMotion};

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
            list.prepare(30, 10, adv, 4, true);
            off = list.offset(/*len*/ 30, /*viewport*/ 10);
        }
        // sel=25 在视口内,且下方仍留 ≥ scrolloff 行(25 - off <= viewport-1-so)。
        assert!(off <= 25 && 25 < off + 10, "选中行可见: off={off}");
        assert!(
            25 - off <= 10 - 1 - 3,
            "选中行下方应留 ≥ scrolloff: off={off}"
        );
    }

    /// `place`:光标落 sel、视口瞬时落位(`Frozen` 读当前位置不推进);首帧即到位无缓动。
    #[test]
    fn place_snaps_viewport_without_glide() {
        let mut list = ScrollList::new();
        list.place(/*sel*/ 20, /*anchor*/ 3);
        // Frozen 读当前位置:snap 到 sel-anchor=17,钳进边界。
        let off = list.offset(/*len*/ 30, /*viewport*/ 10);
        assert_eq!(off, 17, "place 后视口瞬时落在 sel-anchor");
        assert_eq!(list.sel(), 20, "光标落 sel");
    }

    /// `at`:构造即定位(光标 + 视口都落 sel 附近)。
    #[test]
    fn at_positions_on_construct() {
        let list = ScrollList::at(15);
        assert_eq!(list.sel(), 15);
        let off = list.offset(30, 10);
        assert_eq!(off, 15, "视口瞬时落在 sel(anchor=0)");
    }
}
