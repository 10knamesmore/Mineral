//! 列表视口滚动:nvim 手感的 offset 维护 + 缓动平移。
//!
//! 核心约定:**offset 跨帧持久**,光标在 `[offset+scrolloff, offset+视口高-1-scrolloff]`
//! 安全区内移动时视口不动(nvim 默认手感);越出安全区才滚动,且滚动经
//! [`Transition`] 缓动平移(milli-row 定点,与歌词手动滚动同范式)。
//!
//! 不要回到「每帧新建 `TableState::default()` 再 select」的写法——那会让 ratatui
//! 每帧从 offset=0 重新求最小可见滚动,光标永远钉在视口底边、列表粘着光标滚。

use std::cell::RefCell;

use crate::render::anim::Transition;
use crate::runtime::action::ScrollStep;

/// 一步滚动折算成带符号行数(向下为正),行数取 `behavior` 的逐行 / 翻页档步长。
///
/// # Params:
///   - `step`: 方向 + 档位
///   - `behavior`: 交互手感段(步长来源)
///
/// # Return:
///   带符号行数。
pub(crate) fn step_delta(step: ScrollStep, behavior: &mineral_config::BehaviorConfig) -> i64 {
    let line = i64::try_from(*behavior.line_scroll_rows()).unwrap_or(i64::MAX);
    let page = i64::try_from(*behavior.page_scroll_rows()).unwrap_or(i64::MAX);
    match step {
        ScrollStep::LineDown => line,
        ScrollStep::LineUp => -line,
        ScrollStep::PageDown => page,
        ScrollStep::PageUp => -page,
    }
}

/// 给定当前 offset 与光标位置,按 nvim 语义算新 offset。
///
/// 光标在 `[offset+so, offset+viewport-1-so]` 安全区内(含边界)时 offset 不动;
/// 越出哪侧就滚到「光标距该侧恰留 so 行」。`so` 是 `scrolloff` 钳到 `(viewport-1)/2`
/// 的有效值(过大时安全区收缩到近居中但不为空)。结果恒在 `[0, len-viewport]`。
///
/// # Params:
///   - `offset`: 当前视口首行
///   - `sel`: 光标行(调用方保证 `< len`)
///   - `len`: 列表总行数
///   - `viewport`: 视口行数
///   - `scrolloff`: 光标与视口上下边缘的最小行距(配置 `behavior.scrolloff`)
///
/// # Return:
///   新视口首行。
pub(crate) fn clamp_offset(
    offset: usize,
    sel: usize,
    len: usize,
    viewport: usize,
    scrolloff: usize,
) -> usize {
    if viewport == 0 || len <= viewport {
        return 0;
    }
    let so = scrolloff.min(viewport.saturating_sub(1) / 2);
    let mut off = offset;
    if sel < off.saturating_add(so) {
        off = sel.saturating_sub(so);
    } else if sel.saturating_add(so) > off.saturating_add(viewport).saturating_sub(1) {
        off = sel
            .saturating_add(so)
            .saturating_add(1)
            .saturating_sub(viewport);
    }
    off.min(len - viewport)
}

/// 一个列表的视口滚动态:目标 offset 由 [`clamp_offset`] 维护,实际渲染 offset 在
/// `from` → `to` 间按缓动平移(与歌词手动滚动同曲线)。
///
/// 渲染端持 `&AppState` 而平移需要推进/重定目标,故内部 `RefCell`;
/// [`Self::render_offset`] **每帧恰调一次**(渲染即帧推进,列表不在屏上时动画冻结,
/// 重新可见后从冻结位置接着滑)。
pub(crate) struct ListScroll {
    /// 平移态(渲染路径经共享引用更新)。
    glide: RefCell<Glide>,
}

/// `from` → `to` 的一段缓动平移(milli-row = 视口首行 × 1000)。
struct Glide {
    /// 平移起点。每设新目标时置为当前动画位置,连按 / 中途反向都从眼前位置接着滑。
    from_milli: i64,

    /// 平移目标。
    to_milli: i64,

    /// 缓动进度(`expanding`:0 起步推满)。
    glide: Transition,
}

impl Glide {
    /// 当前缓动位置(milli-row):在 `from` → `to` 间按已缓动进度线性插值。
    fn pos_milli(&self) -> i64 {
        let eased = i64::from(self.glide.eased());
        self.from_milli + (self.to_milli - self.from_milli) * eased / 1000
    }

    /// 从当前动画位置朝 `target_milli` 重起一段平移。
    fn retarget(&mut self, target_milli: i64, glide_ticks: u16) {
        self.from_milli = self.pos_milli();
        self.to_milli = target_milli;
        self.glide = Transition::expanding(glide_ticks);
    }
}

/// `usize` 行数 → milli-row(理论越界饱和,实际行数远小于上界)。
fn milli(rows: usize) -> i64 {
    i64::try_from(rows)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000)
}

impl ListScroll {
    /// 新建:视口停在顶,无平移。
    pub(crate) fn new() -> Self {
        Self {
            glide: RefCell::new(Glide {
                from_milli: 0,
                to_milli: 0,
                glide: Transition::expanding(1),
            }),
        }
    }

    /// 渲染入口:推进一拍动画、按光标重算目标 offset(变了就从眼前位置重起平移),
    /// 返回本帧应喂给 `TableState::offset` 的视口首行。
    ///
    /// # Params:
    ///   - `sel`: 光标行
    ///   - `len`: 列表总行数
    ///   - `viewport`: 视口行数
    ///   - `scrolloff`: 光标与视口边缘的最小行距
    ///   - `glide_ticks`: 平移缓动拍数
    ///
    /// # Return:
    ///   本帧视口首行(恒在 `[0, len-viewport]`)。
    pub(crate) fn render_offset(
        &self,
        sel: usize,
        len: usize,
        viewport: usize,
        scrolloff: usize,
        glide_ticks: u16,
    ) -> usize {
        let mut g = self.glide.borrow_mut();
        g.glide.tick();
        let target = usize::try_from(g.to_milli / 1000).unwrap_or(0);
        let new_target = clamp_offset(target, sel, len, viewport, scrolloff);
        if new_target != target {
            g.retarget(milli(new_target), glide_ticks);
            // 新平移当帧就推进一拍:光标行是瞬时移动的,视口同帧起步才不显拖沓,
            // 且 glide_ticks 拍后恰好到位(否则整段平移多占一帧)。
            g.glide.tick();
        }
        // 平移途中的位置可能停留在旧的(已失效的)offset 区间,统一钳回当前上界。
        let pos = usize::try_from((g.pos_milli().max(0) + 500) / 1000).unwrap_or(0);
        pos.min(len.saturating_sub(viewport))
    }

    /// 平移视口目标 `delta_rows` 行(C-d 族:光标与视口同移,保持光标的屏上相对位置)。
    /// 下界钳 0,上界交给 [`Self::render_offset`](视口高度只有渲染端知道)。
    ///
    /// # Params:
    ///   - `delta_rows`: 行数增量(向下为正)
    ///   - `glide_ticks`: 平移缓动拍数
    pub(crate) fn nudge(&self, delta_rows: i64, glide_ticks: u16) {
        let mut g = self.glide.borrow_mut();
        let target = (g.to_milli / 1000)
            .saturating_add(delta_rows)
            .max(0)
            .saturating_mul(1000);
        g.retarget(target, glide_ticks);
    }

    /// 无动画立刻落位(视图切换重置等「不该有滚动感」的场合)。
    ///
    /// # Params:
    ///   - `rows`: 目标视口首行
    pub(crate) fn snap_to(&self, rows: usize) {
        let mut g = self.glide.borrow_mut();
        g.from_milli = milli(rows);
        g.to_milli = g.from_milli;
    }
}

/// 把高亮行钳进 `[offset, offset+viewport-1]`。
///
/// 大跳(`G`/翻页)的平移途中,真实光标可能还在视口之外——而 ratatui `Table` 会
/// 强行调 offset 保证 selected 可见,一帧瞬跳直接打断平移。把传给 `TableState` 的
/// 高亮行钉在就近边缘,视觉上光标贴边领跑、视口追上后自然归位真实行。
///
/// # Params:
///   - `sel`: 真实光标行
///   - `offset`: 本帧视口首行
///   - `viewport`: 视口行数
///
/// # Return:
///   本帧应高亮的行。
pub(crate) fn pin_cursor(sel: usize, offset: usize, viewport: usize) -> usize {
    sel.clamp(offset, offset.saturating_add(viewport.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::proptest;

    use super::{ListScroll, clamp_offset, pin_cursor};

    /// `pin_cursor`:视口内原样返回;视口外钉在就近边缘;零视口退化到 offset。
    #[test]
    fn pin_cursor_clamps_to_viewport() {
        assert_eq!(pin_cursor(15, 10, 10), 15, "视口内不动");
        assert_eq!(
            pin_cursor(29, 3, 9),
            11,
            "光标在视口下方钉在末行 offset+vp-1"
        );
        assert_eq!(pin_cursor(0, 5, 9), 5, "光标在视口上方钉在首行 offset");
        assert_eq!(pin_cursor(7, 4, 0), 4, "零视口退化到 offset");
    }

    /// `clamp_offset` 各分支:安全区内不动 / 上下越界滚动 / 文档首尾钳制 /
    /// 短列表与零视口恒 0 / scrolloff 过大退化到近居中。
    #[test]
    fn clamp_offset_branches() {
        // 安全区 [13, 16](offset=10, vp=10, so=3):光标在内不滚。
        assert_eq!(
            clamp_offset(
                /*offset*/ 10, /*sel*/ 15, /*len*/ 100, /*viewport*/ 10,
                /*scrolloff*/ 3
            ),
            10,
            "安全区内视口不动"
        );
        assert_eq!(
            clamp_offset(10, 13, 100, 10, 3),
            10,
            "恰在上安全边界不滚(边界含)"
        );
        assert_eq!(
            clamp_offset(10, 16, 100, 10, 3),
            10,
            "恰在下安全边界不滚(边界含)"
        );
        // 上越界:sel=12 < 10+3 → offset = 12-3 = 9。
        assert_eq!(clamp_offset(10, 12, 100, 10, 3), 9, "上越界滚到 sel-so");
        // 下越界:sel=17 > 10+9-3 → offset = 17+3+1-10 = 11。
        assert_eq!(
            clamp_offset(10, 17, 100, 10, 3),
            11,
            "下越界滚到 sel+so+1-vp"
        );
        // 文档顶:so 吃不满时钳 0。
        assert_eq!(clamp_offset(5, 1, 100, 10, 3), 0, "近文档顶钳 0");
        // 文档底:offset 钳到 len-vp。
        assert_eq!(clamp_offset(80, 99, 100, 10, 3), 90, "近文档底钳 len-vp");
        // 列表短于视口 / 零视口:恒 0。
        assert_eq!(clamp_offset(3, 4, 5, 10, 3), 0, "短列表恒 0");
        assert_eq!(clamp_offset(3, 4, 100, 0, 3), 0, "零视口恒 0");
        // scrolloff ≥ 半视口:退化为 (vp-1)/2,安全区收缩到近居中但不为空。
        assert_eq!(
            clamp_offset(0, 50, 100, 10, 100),
            45,
            "scrolloff 过大按 (vp-1)/2=4 生效"
        );
    }

    /// 滚到底后向上走:光标在安全区内逐行上移,offset 纹丝不动(nvim 手感的单元版);
    /// 越过上安全边界才开始滚。
    #[test]
    fn clamp_offset_bottom_then_up_keeps_viewport() {
        let bottom = clamp_offset(0, 99, 100, 10, 3);
        assert_eq!(bottom, 90, "跳末行视口到底");
        // 93..=99 都在底部视口的可视区,逐行上移不滚(93 = 90+3 恰为上安全边界)。
        for sel in (93..=99).rev() {
            assert_eq!(
                clamp_offset(bottom, sel, 100, 10, 3),
                90,
                "sel={sel} 不应滚动"
            );
        }
        // 92 < 90+3 → 视口开始上滚。
        assert_eq!(clamp_offset(bottom, 92, 100, 10, 3), 89, "越过安全边界才滚");
    }

    proptest! {
        /// 不变量:结果不越界、选中行可见、scrolloff 在非文档边缘处被满足、幂等。
        #[test]
        fn clamp_offset_invariants(
            offset in 0_usize..200,
            sel in 0_usize..150,
            len in 1_usize..150,
            viewport in 1_usize..40,
            scrolloff in 0_usize..20,
        ) {
            let sel = sel.min(len - 1);
            let got = clamp_offset(offset, sel, len, viewport, scrolloff);
            let max_off = len.saturating_sub(viewport);
            let so = scrolloff.min(viewport.saturating_sub(1) / 2);
            assert!(got <= max_off, "offset 越界: got={got} max={max_off}");
            assert!(got <= sel && sel < got + viewport, "选中行不可见: got={got} sel={sel} vp={viewport}");
            // scrolloff:除非被文档边缘钳住,上下边距都 ≥ so。
            assert!(sel - got >= so || got == 0, "上边距不足: got={got} sel={sel} so={so}");
            assert!(got + viewport - 1 - sel >= so || got == max_off, "下边距不足: got={got} sel={sel} so={so}");
            // 幂等:已满足约束的 offset 不再被改写。
            assert_eq!(clamp_offset(got, sel, len, viewport, scrolloff), got, "应幂等");
        }
    }

    /// 大跳后视口经多帧缓动收敛到目标:轨迹单调、最终精确到位、到位后稳定。
    #[test]
    fn glide_converges_monotonically() {
        let s = ListScroll::new();
        assert_eq!(
            s.render_offset(
                /*sel*/ 0, /*len*/ 100, /*viewport*/ 10, /*scrolloff*/ 3,
                /*glide_ticks*/ 4
            ),
            0,
            "初始在顶"
        );
        // G 跳末行:目标 offset 90,4 拍内单调逼近。
        let mut prev = 0;
        for _ in 0..4 {
            let off = s.render_offset(99, 100, 10, 3, 4);
            assert!(off >= prev, "应单调下滚: {off} >= {prev}");
            assert!(off <= 90, "不过冲: {off}");
            prev = off;
        }
        assert_eq!(prev, 90, "4 拍后到位");
        assert_eq!(s.render_offset(99, 100, 10, 3, 4), 90, "到位后稳定");
    }

    /// 平移中途反向重定目标:从眼前位置接着滑(无跳变),最终收敛到新目标。
    #[test]
    fn glide_retarget_midflight_is_continuous() {
        let s = ListScroll::new();
        // 朝 90 滑两拍(未到位)。
        s.render_offset(99, 100, 10, 3, 8);
        let mid = s.render_offset(99, 100, 10, 3, 8);
        assert!(mid > 0 && mid < 90, "应在途中: {mid}");
        // 反向跳回首行:下一帧不应瞬移,且若干拍后收敛到 0。
        let first = s.render_offset(0, 100, 10, 3, 8);
        assert!(
            first.abs_diff(mid) <= mid,
            "反向首帧从眼前位置接续: mid={mid} first={first}"
        );
        let mut off = first;
        for _ in 0..8 {
            let next = s.render_offset(0, 100, 10, 3, 8);
            assert!(next <= off, "应单调上滚: {next} <= {off}");
            off = next;
        }
        assert_eq!(off, 0, "收敛回顶");
    }

    /// `nudge` 平移视口目标(C-d 族:光标与视口同移的视口半边),渲染端收敛后
    /// 超出文档底的部分被钳回。
    #[test]
    fn nudge_shifts_target_and_clamps() {
        let s = ListScroll::new();
        s.nudge(/*delta_rows*/ 5, /*glide_ticks*/ 2);
        // 光标同步移到 8(由调用方负责),渲染收敛到 offset=5。
        for _ in 0..3 {
            s.render_offset(/*sel*/ 8, 100, 10, 3, 2);
        }
        assert_eq!(s.render_offset(8, 100, 10, 3, 2), 5, "nudge 后收敛到 +5");
        // 负向回推不破坏下界。
        s.nudge(-50, 2);
        for _ in 0..3 {
            s.render_offset(0, 100, 10, 3, 2);
        }
        assert_eq!(s.render_offset(0, 100, 10, 3, 2), 0, "目标钳到 0");
    }

    /// `snap_to`:无动画立刻落位(视图重置用)。
    #[test]
    fn snap_to_lands_immediately() {
        let s = ListScroll::new();
        s.nudge(50, 2);
        s.snap_to(0);
        assert_eq!(s.render_offset(0, 100, 10, 3, 2), 0, "snap 后立刻在顶");
    }

    /// 列表缩短(搜索过滤)后,残留的深 offset 被渲染端钳回新上界。
    #[test]
    fn shrunken_list_clamps_stale_offset() {
        let s = ListScroll::new();
        for _ in 0..5 {
            s.render_offset(99, 100, 10, 3, 2);
        }
        // 过滤后只剩 12 项,光标已被调用方钳到 11。
        let off = s.render_offset(11, 12, 10, 3, 2);
        assert!(off <= 2, "offset 应钳到 len-vp=2: {off}");
    }
}
