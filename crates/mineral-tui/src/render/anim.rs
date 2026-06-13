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

    /// 朝**当前目标**减速的缓动,千分比 `0..=1000`:进场 `1-(1-p)³`(冲向满值再收束)、
    /// 退场 `p³`(立刻收缩再收束到 `0`)——**两个方向都"先快后慢"**。区别于
    /// [`Self::eased`](progress 的固定函数,退场会"先慢后快"显得拖拉)。
    ///
    /// 代价:它依赖目标方向,故**中途反向时值不连续**——两端点(`0`/满)两条曲线同值,
    /// 整段开 / 整段合都不跳;仅在「动画没放完就反向」那一帧可能跳一下。modal 浮层极少
    /// 中途反向,用它换取收回不拖拉;需要反向连续的场景(sweep 等)仍用 [`Self::eased`]。
    pub fn eased_settle(&self) -> u16 {
        if self.leaving() {
            // p³:退场朝 0 减速(先快后慢)。p ≤ 1000 故 p³ ≤ 1e9 不溢出 u32。
            let p = u32::from(self.progress);
            let cube = p * p * p / (u32::from(FULL) * u32::from(FULL));
            u16::try_from(cube).unwrap_or(0)
        } else {
            self.eased()
        }
    }

    /// 当前进度经 cubic **ease-in-out** 映射后的值,千分比 `0..=1000`。关于中点对称:
    /// 两端减速、中段快。区别于 [`Self::eased`](单向 ease-out)——它对进度**增减两个
    /// 方向都"减速到位"**,故左右 sweep 来回切换体感一致、无结尾冲刺。仍是进度的固定
    /// 函数,打断反向时值连续不跳变。
    pub fn eased_in_out(&self) -> u16 {
        ease_in_out(self.progress)
    }
}

/// 二态开关 + 过渡。**逻辑「开/关」不单独存,从内部 [`Transition`] 的目标方向派生**——
/// `enter` 朝「开」(满值)、`leave` 朝「关」(`0`),故 `target` 即逻辑态,渲染读 eased 进度。
///
/// 用于「立即翻转的逻辑标志 + 缓动到位的渲染位置」这一组合(全屏进退 / 失焦变灰等):
/// 标志供按键路由 / 上报,位置供渲染;两者由同一 `Transition` 表达,消除「bool 与
/// 动画目标必须手动保持同步」的冗余。
#[derive(Clone, Copy, Debug)]
pub struct Toggle(Transition);

impl Toggle {
    /// 构造一个停在「关」(`0`)的开关。`ticks` 为从关到开所需拍数(决定时长)。
    pub fn new(ticks: u16) -> Self {
        Self(Transition::new(ticks))
    }

    /// 逻辑态:是否「开」(目标朝满值)。立即反映 [`Self::set`],不等动画放完。
    pub fn on(&self) -> bool {
        !self.0.leaving()
    }

    /// 置逻辑态:`true` 朝「开」推进、`false` 朝「关」。中途反向只改目标不跳变。
    pub fn set(&mut self, on: bool) {
        if on {
            self.0.enter();
        } else {
            self.0.leave();
        }
    }

    /// 翻转逻辑态。
    pub fn toggle(&mut self) {
        self.set(!self.on());
    }

    /// 推进过渡一拍。
    pub fn tick(&mut self) {
        self.0.tick();
    }

    /// 当前进度经 ease-in-out 映射的渲染值,千分比 `0..=1000`。
    pub fn eased_in_out(&self) -> u16 {
        self.0.eased_in_out()
    }

    /// 进度处于「关」端点(`0`):渲染可退化为单态、跳过离屏合成。
    pub fn at_min(&self) -> bool {
        self.0.at_min()
    }

    /// 进度处于「开」端点(满值):渲染可退化为目标单态。
    pub fn at_max(&self) -> bool {
        self.0.at_max()
    }

    /// 过渡是否已抵达目标(动画放完,稳态)。
    pub fn settled(&self) -> bool {
        self.0.settled()
    }
}

/// 动画时长(毫秒)→ tick 数:按主循环帧间隔四舍五入,至少 1(0ms 也占一拍,
/// 语义 = "一帧到位")。配置面只写毫秒,运行时统一经此换算 —— 改 `frame_tick_ms`
/// 不改动画的真实时长。
///
/// # Params:
///   - `ms`: 动画时长(毫秒)
///   - `tick_ms`: 主循环帧间隔(毫秒,配置 `animation.frame_tick_ms`)
///
/// # Return:
///   tick 数,`1..=u32::MAX`。
pub(crate) fn ticks32_from_ms(ms: u32, tick_ms: u64) -> u32 {
    let tick = tick_ms.max(1);
    let n = (u64::from(ms) + tick / 2) / tick;
    u32::try_from(n.max(1)).unwrap_or(u32::MAX)
}

/// [`ticks32_from_ms`] 的 u16 收窄版,喂 [`Transition::new`] 等 u16 拍数构造口
/// (超界饱和到 `u16::MAX`)。
pub(crate) fn ticks16_from_ms(ms: u32, tick_ms: u64) -> u16 {
    u16::try_from(ticks32_from_ms(ms, tick_ms)).unwrap_or(u16::MAX)
}

/// 在 `a`、`b` 间按千分比 `t`(`0..=1000`)定点插值:`a + round((b-a)*t/1000)`。全程 `i32`,
/// 不碰 `as` 强转;结果 clamp 进 `u16` 范围。**四舍五入(非截断)**:让「跨整格」的位移落在
/// 形变中段、而非把整格位移全挤到末帧(否则收尾静止时会突兀跳一格)。布局形变的矩形插值与
/// 通知层的锚点过渡共用这一个实现。
pub(crate) fn lerp_u16(a: u16, b: u16, t: u16) -> u16 {
    let (a, b, t) = (i32::from(a), i32::from(b), i32::from(t.min(1000)));
    let scaled = (b - a) * t;
    // 朝零外侧 ±500 再整除 = 四舍五入(half away from zero),正负对称。
    let rounded = (scaled + if scaled >= 0 { 500 } else { -500 }) / 1000;
    u16::try_from((a + rounded).clamp(0, i32::from(u16::MAX))).unwrap_or(0)
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
    use super::{FULL, Transition, ease_in_out, ticks16_from_ms};

    /// `ticks16_from_ms`:默认值精确换算(288/16=18、96/16=6)、四舍五入、
    /// 下限 1、超界饱和到 `u16::MAX`、`tick_ms=0` 不除零。
    #[test]
    fn ticks16_from_ms_rounds_floors_and_saturates() {
        assert_eq!(ticks16_from_ms(288, 16), 18, "默认 transition_ms 精确换算");
        assert_eq!(ticks16_from_ms(96, 16), 6, "默认 toast_anim_ms 精确换算");
        assert_eq!(ticks16_from_ms(100, 16), 6, "6.25 拍应四舍五入到 6");
        assert_eq!(ticks16_from_ms(104, 16), 7, "6.5 拍应四舍五入到 7");
        assert_eq!(ticks16_from_ms(0, 16), 1, "0ms 也占一拍(一帧到位)");
        assert_eq!(ticks16_from_ms(5, 16), 1, "不足半拍也至少 1");
        assert_eq!(ticks16_from_ms(u32::MAX, 1), u16::MAX, "超界饱和");
        assert_eq!(ticks16_from_ms(160, 0), 160, "tick_ms=0 按 1ms 兜底不除零");
    }

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

    /// `eased_settle`:进场与 `eased` 同曲线;退场改走 `p³`,**先快后慢收向 0**——
    /// 同一进度下退场值低于 `eased`(更早收缩),且两端点两路一致(整段无跳)。
    #[test]
    fn eased_settle_decelerates_into_both_targets() {
        // 进场方向:与 eased 完全一致。
        let mut up = Transition::new(FULL);
        up.enter();
        for _ in 0..FULL {
            up.tick();
            assert_eq!(up.eased_settle(), up.eased(), "进场两者同曲线");
        }
        // 退场方向:p³,先快后慢收向 0。
        let mut down = Transition::new(FULL);
        down.enter();
        for _ in 0..FULL {
            down.tick();
        }
        down.leave();
        assert_eq!(down.eased_settle(), FULL, "退场起点(满)两路一致");
        let mut prev = FULL;
        for _ in 0..FULL {
            down.tick();
            let v = down.eased_settle();
            assert!(v <= prev, "退场单调收缩: {v} <= {prev}");
            // 退场早期(进度仍高)就已明显收缩,不像 eased 那样赖在满值附近。
            assert!(v <= down.eased(), "退场更早收缩: settle {v} ≤ eased");
            prev = v;
        }
        assert_eq!(down.eased_settle(), 0, "退场收到 0");
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
