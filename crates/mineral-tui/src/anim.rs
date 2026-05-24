//! 过渡动画基元。逐 tick 推进的归一化进度,供 modal 浮层弹出/收起等过渡复用。

/// 进度满值(千分比)。`Transition` 用整数千分比表示 `0.0..=1.0`,避免浮点转换
/// (项目禁 `as` 强转,`f32` 又无到整数的 `TryFrom`),与 transport 进度同走定点。
const FULL: u16 = 1000;

/// ease-out-back 的回弹系数 `c1`(标准 1.70158)放大千分。
const BACK_C1: i64 = 1702;

/// ease-out-back 的回弹系数 `c3 = c1 + 1`(标准 2.70158)放大千分。
const BACK_C3: i64 = 2702;

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

    /// 当前进度经缓动映射后的值,千分比;按方向择曲线 —— **进场**(目标满值)走
    /// ease-out-back(尾段轻微冲过满值再弹回,~1000..=1090),**退场**(目标 0)走
    /// ease-out-cubic(干净收拢,`0..=1000` 单调)。
    ///
    /// 两段分治意在避免「单曲线纯映射」时退场也在高进度区过冲、面板先膨胀再收缩的
    /// 怪异观感(对应 noice 通知 in/out 分阶段的做法)。进场过冲值可 `> 1000`,
    /// 调用方据此把面板画得短暂略大于目标尺寸,呈现「弹出」。
    pub fn eased(&self) -> u16 {
        if self.target >= FULL {
            self.ease_out_back()
        } else {
            self.ease_out_cubic()
        }
    }

    /// cubic ease-out:`1 - (1 - p)^3`,千分比 `0..=1000` 单调。全程 u32 定点。
    fn ease_out_cubic(&self) -> u16 {
        let inv = u32::from(FULL - self.progress);
        let cube = inv * inv * inv / (u32::from(FULL) * u32::from(FULL));
        u16::try_from(u32::from(FULL) - cube).unwrap_or(FULL)
    }

    /// ease-out-back:`1 + c3·u³ + c1·u²`(`u = p - 1`),尾段冲过 `1000` 再回落。
    /// 端点钉在 `0` / `1000`,峰值约 `1090`。全程 i64 定点(`c3·e³` 最大 ~2.7e12)。
    fn ease_out_back(&self) -> u16 {
        let e = i64::from(FULL - self.progress);
        let term2 = BACK_C1 * e * e / 1_000_000;
        let term3 = BACK_C3 * e * e * e / 1_000_000_000;
        let r = (i64::from(FULL) + term2 - term3).clamp(0, i64::from(u16::MAX));
        u16::try_from(r).unwrap_or(FULL)
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

    /// 进场(ease-out-back):端点钉在 0 / 满值,中途冲过满值再回落(回弹)。
    #[test]
    fn enter_overshoots_then_settles() {
        let mut t = Transition::new(6);
        t.enter();
        assert!(t.active());
        let mut peak = 0;
        for _ in 0..6 {
            t.tick();
            peak = peak.max(t.eased());
        }
        // 到顶精确落在满值。
        assert_eq!(t.eased(), FULL);
        // 过程中确有冲过满值的回弹帧。
        assert!(peak > FULL, "expected overshoot, peak = {peak}");
        // 稳态后不再变化。
        for _ in 0..3 {
            t.tick();
        }
        assert_eq!(t.eased(), FULL);
    }

    /// 退场(ease-out-cubic):单调回落到 0,全程不超过满值(无膨胀)。
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
            assert!(v <= FULL, "close must not balloon past full: {v}");
            prev = v;
        }
        assert_eq!(t.eased(), 0);
        assert!(!t.active());
    }
}
