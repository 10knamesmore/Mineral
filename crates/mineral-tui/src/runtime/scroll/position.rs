//! 列表光标在全列表中的相对位置与缓动，以及吸住后的颜色过渡；都不依赖当前视口或面板高度。

use std::cell::RefCell;

use crate::render::anim::Transition;

/// 全列表位置的归一化满值；保留长列表中不足一个终端格的移动精度。
pub(crate) const POSITION_SCALE: u32 = 1_000_000;

/// 吸附满值（千分比）：在播格完全取光标色，光晕完全熄灭。
pub(crate) const MAGNET_FULL: u16 = 1000;

/// 把列表下标映射到首项为零、末项为满值的位置；空列表没有位置，单项贴顶。
///
/// # Params:
///   - `index`: 当前显示顺序中的下标，越界时钳到末项。
///   - `total`: 当前显示列表的总项数。
pub(crate) fn relative_position(index: usize, total: usize) -> Option<u32> {
    let last = total.checked_sub(1)?;
    scaled_position(
        u128::try_from(index.min(last)).ok()?,
        u128::try_from(last).ok()?,
    )
}

/// 第 `boundary` 条等分边界的位置：它上下两项在中点分界，首末边界贴住两端。
///
/// 与 [`relative_position`] 是同一投影：第 `index` 项占
/// `boundary_position(index, total)..=boundary_position(index + 1, total)`，
/// 相邻两项的区间首尾相接，且各自包住自己的点位。
///
/// # Params:
///   - `boundary`: 边界上方的项序号；`0` 是列表最顶端，大于等于 `total` 时贴住满值。
///   - `total`: 当前显示列表的总项数；空列表没有边界，单项占满整条。
pub(crate) fn boundary_position(boundary: usize, total: usize) -> Option<u32> {
    let last = total.checked_sub(1)?;
    if boundary == 0 {
        return Some(0);
    }
    if boundary >= total {
        return Some(POSITION_SCALE);
    }
    // 相邻两项的中点：(2 * boundary - 1) / (2 * last)。
    let numerator = 2 * u128::try_from(boundary).ok()? - 1;
    scaled_position(numerator, 2 * u128::try_from(last).ok()?)
}

/// 把 `numerator / denominator` 映射到首项为零、末项为满值的位置；分母为零时只剩顶端。
///
/// # Params:
///   - `numerator`: 分子。
///   - `denominator`: 分母。
fn scaled_position(numerator: u128, denominator: u128) -> Option<u32> {
    if denominator == 0 {
        return Some(0);
    }
    u32::try_from(numerator * u128::from(POSITION_SCALE) / denominator).ok()
}

/// 跟随列表光标的显示位置；首帧与列表长度变化直接定位，导航时从眼前位置缓动。
#[derive(Clone, Default)]
pub(crate) struct ListPosition {
    /// 尚未显示或列表已重置时为空；绘制路径通过共享引用推进。
    active: RefCell<Option<PositionTransition>>,
}

/// 同一长度的列表内，一次从显示位置到逻辑光标的移动。
#[derive(Clone)]
struct PositionTransition {
    /// 当前归一化坐标对应的列表长度；过滤改变长度后旧坐标不再适用。
    total: usize,

    /// 本段动画的起点，单位为 POSITION_SCALE 的比例。
    from: u32,

    /// 本段动画的目标，单位与起点相同。
    target: u32,

    /// 只保存动画相位；每次推进前用现行配置重设速度。
    transition: Transition,
}

impl PositionTransition {
    /// 对起点与目标做有界整数插值，中途反向时以该值作为下一段起点。
    fn position(&self) -> u32 {
        let progress = u32::from(self.transition.eased());
        (self.from * (1000 - progress) + self.target * progress) / 1000
    }
}

impl ListPosition {
    /// 丢弃旧显示位置；下一次绘制直接落在新列表的光标处。
    pub(crate) fn reset(&mut self) {
        *self.active.get_mut() = None;
    }

    /// 在稳态绘制中推进一拍；每个列表每帧调用一次。
    ///
    /// # Params:
    ///   - `selected`: 当前显示顺序中的光标下标。
    ///   - `total`: 当前显示列表长度。
    ///   - `ticks`: 从当前配置折算的移动拍数，热更时保留相位并立即改变后续速度。
    pub(crate) fn advance(&self, selected: usize, total: usize, ticks: u16) -> Option<u32> {
        let mut active = self.active.borrow_mut();
        let Some(target) = relative_position(selected, total) else {
            *active = None;
            return None;
        };
        if active.as_ref().is_none_or(|state| state.total != total) {
            *active = Some(PositionTransition {
                total,
                from: target,
                target,
                transition: Transition::expanding(ticks),
            });
        }
        let state = active.as_mut()?;
        if state.target != target {
            state.from = state.position();
            state.target = target;
            state.transition = Transition::expanding(ticks);
        }
        state.transition.retempo(ticks);
        state.transition.tick();
        Some(state.position())
    }

    /// 只读显示当前位置；离屏合成与尺寸形变不推进或重定动画。
    ///
    /// # Params:
    ///   - `selected`: 首次绘制或列表长度变化时用于直接定位的光标。
    ///   - `total`: 当前显示列表长度。
    pub(crate) fn frozen(&self, selected: usize, total: usize) -> Option<u32> {
        let active = self.active.borrow();
        match active.as_ref() {
            Some(state) if state.total == total => Some(state.position()),
            _ => relative_position(selected, total),
        }
    }
}

/// 光标吸进在播标记之后的颜色过渡进度；`0` = 在播色 + 光晕照常，[`MAGNET_FULL`] = 光标色 + 光晕熄灭。
#[derive(Clone, Default)]
pub(crate) struct MagnetProgress {
    /// 尚未吸过或列表已重置时为空，下一帧直接从目标值开始；否则保存当前段的相位。
    active: RefCell<Option<MagnetTransition>>,
}

/// 一次进 / 出吸附区的颜色过渡。
#[derive(Clone)]
struct MagnetTransition {
    /// 本段起点，单位为千分比。
    from: u16,

    /// 本段目标，单位为千分比。
    target: u16,

    /// 只保存相位；每次推进前用现行配置重设速度。
    transition: Transition,
}

impl MagnetTransition {
    /// 对起点与目标做有界整数插值；中途反向时以该值作为下一段起点。
    fn progress(&self) -> u16 {
        let progress = u32::from(self.transition.raw());
        let from = u32::from(self.from);
        let target = u32::from(self.target);
        u16::try_from((from * (1000 - progress) + target * progress) / 1000).unwrap_or(0)
    }
}

impl MagnetProgress {
    /// 丢弃进度；下一次绘制直接落在目标值，不补一段过渡。
    pub(crate) fn reset(&mut self) {
        *self.active.get_mut() = None;
    }

    /// 在稳态绘制中推进一拍并读取进度。
    ///
    /// # Params:
    ///   - `absorbed`: 本帧光标是否落在在播标记的吸附区里。
    ///   - `ticks`: 从当前配置折算的过渡拍数，热更时保留相位并立即改变后续速度。
    ///
    /// # Return:
    ///   `0..=1000` 的千分比，[`MAGNET_FULL`] 表示完全吸住。
    pub(crate) fn advance(&self, absorbed: bool, ticks: u16) -> u16 {
        let target = if absorbed { MAGNET_FULL } else { 0 };
        let mut active = self.active.borrow_mut();
        let state = active.get_or_insert_with(|| MagnetTransition {
            from: target,
            target,
            transition: Transition::expanding(ticks),
        });
        if state.target != target {
            state.from = state.progress();
            state.target = target;
            state.transition = Transition::expanding(ticks);
        }
        state.transition.retempo(ticks);
        state.transition.tick();
        state.progress()
    }

    /// 只读当前进度；离屏合成与尺寸形变不推进动画。
    pub(crate) fn frozen(&self) -> u16 {
        self.active
            .borrow()
            .as_ref()
            .map_or(0, MagnetTransition::progress)
    }
}
