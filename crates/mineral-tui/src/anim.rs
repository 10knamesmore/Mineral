//! 过渡动画基元。逐 tick 推进的归一化进度,供 modal 浮层弹出/收起等过渡复用。

/// 进度满值(千分比)。`Transition` 用整数千分比表示 `0.0..=1.0`,避免浮点转换
/// (项目禁 `as` 强转,`f32` 又无到整数的 `TryFrom`),与 transport 进度同走定点。
const FULL: u16 = 1000;

/// 一个 `0..=1000`(千分比)的过渡进度,逐 tick 朝目标推进,可按缓动曲线取值。
///
/// 面板弹出/收起、淡入淡出等 modal 过渡的通用基元,queue 浮层是首个调用方。
/// 推进按「每 tick 固定步长」(不看 wall-clock),与 [`crate::components::spectrum`]
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

    /// 当前进度经 cubic ease-out 映射后的值,千分比 `0..=1000`。快进慢出,**无回弹/
    /// 过冲**;进场退场同一条曲线(进度单调 → 值单调),不会超过满值。
    pub fn eased(&self) -> u16 {
        // 1 - (1 - p)^3,p 取千分比;全程 u32 定点,1000^3 = 1e9 不溢出。
        let inv = u32::from(FULL - self.progress);
        let cube = inv * inv * inv / (u32::from(FULL) * u32::from(FULL));
        u16::try_from(u32::from(FULL) - cube).unwrap_or(FULL)
    }
}

#[cfg(test)]
mod tests {
    use super::{FULL, Transition};

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
}
