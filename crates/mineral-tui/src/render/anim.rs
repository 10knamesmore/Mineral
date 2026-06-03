//! 过渡动画基元。逐 tick 推进的归一化进度,供 modal 浮层弹出/收起等过渡复用。

/// 进度满值(千分比)。`Transition` 用整数千分比表示 `0.0..=1.0`,避免浮点转换
/// (项目禁 `as` 强转,`f32` 又无到整数的 `TryFrom`),与 transport 进度同走定点。
const FULL: u16 = 1000;

/// 一个 `0..=1000`(千分比)的过渡进度,逐 tick 朝目标推进,可按缓动曲线取值。
///
/// 面板弹出/收起、淡入淡出等 modal 过渡的通用基元,queue 浮层是首个调用方。
/// 推进按「每 tick 固定步长」(不看 wall-clock),与 [`crate::components::layout::spectrum`]
/// 的动画同范式 —— 确定性、可单测。本身不引用任何 widget / 渲染类型,纯数值。
#[derive(Clone, Copy, Debug)]
pub struct Transition {
    /// 当前进度,千分比 `0..=1000`。
    progress: u16,

    /// 目标:进场 = `1000`,退场 = `0`。
    target: u16,

    /// 每 tick 步长(千分比)。
    step: u16,
}

impl Transition {
    /// 构造一个停在 `0`(已收起)的过渡。
    ///
    /// # Params:
    ///   - `ticks`: 从 `0` 走到满值所需的 tick 数(决定时长);向上取整保证按时到顶。
    pub fn new(ticks: u16) -> Self {
        Self {
            progress: 0,
            target: 0,
            step: FULL.div_ceil(ticks.max(1)),
        }
    }

    /// 构造一个**已完全展开**(满值)、即将收起的过渡:[`Self::tick`] 把进度从满推向 `0`。
    /// 用于整屏退出收缩等「从满开始收」的场景(配合 [`Self::eased`] 得到加速收束感)。
    ///
    /// # Params:
    ///   - `ticks`: 从满值走到 `0` 所需的 tick 数(决定时长)。
    pub fn collapsing(ticks: u16) -> Self {
        Self {
            progress: FULL,
            target: 0,
            step: FULL.div_ceil(ticks.max(1)),
        }
    }

    /// 构造一个**完全收起**(零)、即将展开的过渡:[`Self::tick`] 把进度从 `0` 推向满值。
    /// 用于整屏启动扩大等「从零开始展开」的场景(配合 [`Self::eased`] 得到加速铺开感),
    /// 与 [`Self::collapsing`] 反向对称。
    ///
    /// # Params:
    ///   - `ticks`: 从 `0` 走到满值所需的 tick 数(决定时长)。
    pub fn expanding(ticks: u16) -> Self {
        Self {
            progress: 0,
            target: FULL,
            step: FULL.div_ceil(ticks.max(1)),
        }
    }

    /// 开始进场:目标置满,后续 [`Self::tick`] 把进度推向满值。
    pub fn enter(&mut self) {
        self.target = FULL;
    }

    /// 开始退场:目标置零,后续 [`Self::tick`] 把进度推向 `0`。
    pub fn leave(&mut self) {
        self.target = 0;
    }

    /// 朝目标推进一步,clamp 在 `[0, 1000]`。已到目标则为空操作。
    pub fn tick(&mut self) {
        if self.progress < self.target {
            self.progress = self.progress.saturating_add(self.step).min(self.target);
        } else if self.progress > self.target {
            self.progress = self.progress.saturating_sub(self.step).max(self.target);
        }
    }

    /// 是否仍需渲染:进度未归零,或正朝进场推进。完全收起且无进场目标时为 `false`。
    pub fn active(&self) -> bool {
        self.progress > 0 || self.target > 0
    }

    /// 是否正朝收起推进(目标为零)。栈用它判定浮层"逻辑已关、视觉还在收尾",
    /// 据此把键盘焦点收回、并在归零后真正移除。
    pub fn leaving(&self) -> bool {
        self.target == 0
    }

    /// 进度是否已抵达目标(转场收尾)。进场 settled 于满值、退场 settled 于 `0`;
    /// 整屏转场据此判定「动画放完」——区别于 [`Self::active`](进场到顶后仍 active)。
    pub fn settled(&self) -> bool {
        self.progress == self.target
    }

    /// 进度处于起点(`0`)。视图过渡据此退化为「起始视图」,免去离屏合成开销。
    pub fn at_min(&self) -> bool {
        self.progress == 0
    }

    /// 进度处于满值终点。视图过渡据此退化为「目标视图」。
    pub fn at_max(&self) -> bool {
        self.progress == FULL
    }

    /// 当前进度经 cubic ease-out 映射后的值,千分比 `0..=1000`。快进慢出,**无回弹/
    /// 过冲**;进场退场同一条曲线(进度单调 → 值单调),不会超过满值。
    pub fn eased(&self) -> u16 {
        // 1 - (1 - p)^3,p 取千分比;全程 u32 定点,1000^3 = 1e9 不溢出。
        let inv = u32::from(FULL - self.progress);
        let cube = inv * inv * inv / (u32::from(FULL) * u32::from(FULL));
        u16::try_from(u32::from(FULL) - cube).unwrap_or(FULL)
    }

    /// 当前进度经 cubic **ease-in-out** 映射后的值,千分比 `0..=1000`。关于中点对称:
    /// 两端减速、中段快。区别于 [`Self::eased`](单向 ease-out)——它对进度**增减两个
    /// 方向都"减速到位"**,故左右 sweep 来回切换体感一致、无结尾冲刺。仍是进度的固定
    /// 函数,打断反向时值连续不跳变。
    pub fn eased_in_out(&self) -> u16 {
        ease_in_out(self.progress)
    }
}

/// cubic **ease-in-out** 映射:进度千分比 `progress`(`0..=1000`)→ 缓动值千分比
/// (`0..=1000`)。关于中点对称——两端减速、中段快,对进度增减两个方向都"减速到位"。
/// 单调不过冲。[`Transition::eased_in_out`] 与歌词平滑滚动共用这一条曲线。
///
/// # Params:
///   - `progress`: 线性进度千分比,`0..=1000`(超出按上界处理)。
///
/// # Return:
///   缓动后的千分比,`0..=1000`。
pub fn ease_in_out(progress: u16) -> u16 {
    let p = u32::from(progress.min(FULL));
    let full = u32::from(FULL);
    let half = full / 2;
    // p<半: 4p³;p≥半: 1 - (2-2p)³/2(归一化后)。全程 u32 定点:
    // 4p³ < 4·500³ = 5e8、(2·FULL)³ 段 t³ ≤ 1e9,均不溢出。
    let v = if p < half {
        4 * p * p * p / (full * full)
    } else {
        let t = 2 * full - 2 * p; // 0..=FULL
        full - t * t * t / (2 * full * full)
    };
    u16::try_from(v).unwrap_or(FULL)
}

#[cfg(test)]
mod tests {
    use super::{FULL, Transition, ease_in_out};

    /// 自由函数 `ease_in_out`:端点(0/满)、中点(过 0.5,0.5),全程单调不过冲。
    /// 与 [`Transition::eased_in_out`] 同一条曲线(后者委托它)。
    #[test]
    fn free_ease_in_out_endpoints_and_monotonic() {
        assert_eq!(ease_in_out(0), 0, "起点");
        assert_eq!(ease_in_out(FULL), FULL, "终点");
        assert_eq!(ease_in_out(FULL / 2), FULL / 2, "cubic ease-in-out 过中点");
        let mut prev = 0;
        for p in 0..=FULL {
            let v = ease_in_out(p);
            assert!(v >= prev, "单调不降: p={p} v={v} prev={prev}");
            assert!(v <= FULL, "不过冲: {v}");
            prev = v;
        }
    }

    /// `Transition::eased_in_out` 与自由函数对任意进度一致(委托关系守卫)。
    #[test]
    fn method_delegates_to_free_fn() {
        let mut t = Transition::new(FULL);
        t.enter();
        for _ in 0..FULL {
            t.tick();
            assert_eq!(t.eased_in_out(), ease_in_out(t.progress));
        }
    }

    /// 新建即收起态:不 active,缓动值为 0。
    #[test]
    fn new_is_inactive() {
        let t = Transition::new(6);
        assert!(!t.active());
        assert_eq!(t.eased(), 0);
    }

    /// 进场:单调逼近满值,全程不超过满值(无回弹/过冲),到顶后稳定。
    #[test]
    fn enter_rises_monotonically_to_full() {
        let mut t = Transition::new(6);
        t.enter();
        assert!(t.active());
        let mut prev = 0;
        for _ in 0..6 {
            t.tick();
            let v = t.eased();
            assert!(v >= prev, "expected monotonic rise: {v} >= {prev}");
            assert!(v <= FULL, "must not overshoot past full: {v}");
            prev = v;
        }
        assert_eq!(t.eased(), FULL);
        for _ in 0..3 {
            t.tick();
        }
        assert_eq!(t.eased(), FULL);
    }

    /// 退场:单调回落到 0,全程不超过满值。
    #[test]
    fn leave_shrinks_monotonically_to_zero() {
        let mut t = Transition::new(6);
        t.enter();
        for _ in 0..6 {
            t.tick();
        }
        t.leave();
        let mut prev = FULL;
        for _ in 0..6 {
            t.tick();
            let v = t.eased();
            assert!(v <= prev, "expected monotonic shrink: {v} <= {prev}");
            assert!(v <= FULL, "must not exceed full: {v}");
            prev = v;
        }
        assert_eq!(t.eased(), 0);
        assert!(!t.active());
    }

    /// 启动扩大:从 0 单调升到满值,非退场;到顶后 `settled`、缓动值为满。
    #[test]
    fn expanding_rises_to_full_not_leaving() {
        let mut t = Transition::expanding(4);
        assert!(!t.leaving(), "扩大是进场,不应判为 leaving");
        assert!(!t.settled(), "起步未到目标");
        assert_eq!(t.eased(), 0);
        let mut prev = 0;
        for _ in 0..4 {
            t.tick();
            let v = t.eased();
            assert!(v >= prev, "expected monotonic rise: {v} >= {prev}");
            assert!(v <= FULL, "must not overshoot past full: {v}");
            prev = v;
        }
        assert!(t.settled(), "推满后应 settled");
        assert_eq!(t.eased(), FULL);
    }

    /// ease-in-out:过端点(0/满)与中点(满/2),关于中点对称,且单调不过冲。
    #[test]
    fn eased_in_out_symmetric_and_monotonic() {
        let mut t = Transition::new(FULL); // step=1,progress 可逐点走
        t.enter();
        assert_eq!(t.eased_in_out(), 0, "起点");
        let mut prev = 0;
        let mut samples = Vec::<u16>::new();
        for _ in 0..FULL {
            t.tick();
            let v = t.eased_in_out();
            assert!(v >= prev, "单调不降: {v} >= {prev}");
            assert!(v <= FULL, "不过冲: {v}");
            prev = v;
            samples.push(v);
        }
        assert_eq!(t.eased_in_out(), FULL, "终点到满");
        // 中点对称:f(p) + f(FULL-p) ≈ FULL(定点除法允许 ±2 误差)。
        if let (Some(&lo), Some(&hi)) = (
            samples.get(usize::from(FULL / 4 - 1)),
            samples.get(usize::from(FULL - FULL / 4 - 1)),
        ) {
            let sum = u32::from(lo) + u32::from(hi);
            assert!(
                sum.abs_diff(u32::from(FULL)) <= 2,
                "应关于中点对称: {lo} + {hi} ≈ {FULL}"
            );
        }
    }

    /// 退出收缩:从满值收到 0,是退场;到底后 `settled`、缓动值为 0。
    #[test]
    fn collapsing_settles_at_zero() {
        let mut t = Transition::collapsing(4);
        assert!(t.leaving(), "收缩是退场");
        assert!(!t.settled(), "起步未到目标");
        for _ in 0..4 {
            t.tick();
        }
        assert!(t.settled(), "收到底后应 settled");
        assert!(t.leaving());
        assert_eq!(t.eased(), 0);
    }
}
