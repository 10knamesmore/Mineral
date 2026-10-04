//! 组件绘制依赖、缓存与按遮挡顺序排列的绘制计划。

use super::component_id::ComponentId;
use super::damage::Damage;

use std::any::Any;
use std::cell::Cell;
use std::sync::Arc;

use ratatui::{Frame, buffer::Buffer, layout::Rect};

use crate::components::frame::FrameEnv;

/// 与组件实例同生共死的绘制缓存；内部可变性只保存透明派生结果。
/// `Cell::take` 不持有运行时借用，嵌套绘制也不会产生借用冲突。
pub(crate) struct PaintCache {
    /// 随缓存移动保留的实例编号；组件重新构造时重新发号。
    id: ComponentId,

    /// 最近一次输入与可复用的绘制结果。
    memory: Cell<Option<PaintMemory>>,
}

impl Default for PaintCache {
    fn default() -> Self {
        Self {
            id: ComponentId::new(),
            memory: Cell::default(),
        }
    }
}

impl Clone for PaintCache {
    /// 复制交互状态不共享绘制结果或挂载身份。
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for PaintCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaintCache").finish_non_exhaustive()
    }
}

/// 配置通过整树替换更新；保留 Arc 防止地址复用误判。
struct ConfigIdentity(
    /// 上次绘制时的配置身份，不作为组件的配置数据源。
    Arc<mineral_config::Config>,
);

impl Clone for ConfigIdentity {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl PartialEq for ConfigIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// 一次完整绘制的输入与结果。
#[derive(Default)]
struct PaintMemory {
    /// 按组件声明顺序保留的依赖，不保存整个应用状态。
    inputs: Vec<Box<dyn Any>>,

    /// 声明的输入是否已经绘制；取消尚未绘制的计划会保持失效。
    valid: bool,

    /// 保存的结果是否仍对应当前输入及下层合成。
    reusable: bool,

    /// 输入稳定后保存完整区域；失效时保留分配，避免滚动中反复分配。
    output: Option<Buffer>,
}

/// 正在为一个组件声明本次依赖；丢弃时把内存交回所属组件。
pub(crate) struct PaintInputs<'a> {
    /// 持有上次结果的组件。
    cache: &'a PaintCache,

    /// 正在核对的依赖与结果。
    memory: PaintMemory,

    /// 已声明的依赖数。
    index: usize,

    /// 本次绘制位置，位移也会使结果失效。
    area: Rect,
}

impl PaintCache {
    /// 开始声明组件依赖；尺寸、当前配置与有效主题属于所有组件的共同输入。
    pub(crate) fn inputs(&self, area: Rect, env: FrameEnv<'_>) -> PaintInputs<'_> {
        let mut inputs = PaintInputs {
            cache: self,
            memory: self.memory.take().unwrap_or_default(),
            index: 0,
            area,
        };
        let mut dependencies = inputs.dependencies();
        dependencies.observe(&area);
        dependencies.observe(&ConfigIdentity(Arc::clone(env.config)));
        dependencies.observe(env.theme);
        inputs
    }
}

impl PaintInputs<'_> {
    /// 本组件读取和写入 cell 的完整区域；合成失效按区域相交传播。
    pub(crate) fn area(&self) -> Rect {
        self.area
    }

    /// 借出依赖记录能力；组件无法通过它操作缓存结果或登记绘制。
    pub(crate) fn dependencies(&mut self) -> Dependencies<'_> {
        Dependencies {
            memory: &mut self.memory,
            index: &mut self.index,
            area: self.area,
        }
    }
}

/// 本组件的绘制依赖记录器；只比较和保存依赖值，不暴露缓存或提交能力。
pub(crate) struct Dependencies<'a> {
    /// 所属实例上次完成绘制的输入及有效性。
    memory: &'a mut PaintMemory,

    /// 本次已声明的依赖位置。
    index: &'a mut usize,

    /// 本组件的完整读写区域，供可见窗口等依赖计算使用。
    area: Rect,
}

impl Dependencies<'_> {
    /// 返回本次绘制声明的完整区域，不提供修改几何或缓存的能力。
    pub(crate) fn area(&self) -> Rect {
        self.area
    }

    /// 声明一个绘制所读的值；相等时既不复制它，也不使缓存失效。
    /// 各分支须完整声明自己的输入，依赖数量或类型变化同样会失效。
    pub(crate) fn observe<T: Clone + PartialEq + 'static>(&mut self, value: &T) {
        let memory = &mut self.memory;
        let same = memory
            .inputs
            .get(*self.index)
            .is_some_and(|previous| previous.downcast_ref::<T>() == Some(value));
        if !same {
            memory.valid = false;
            if let Some(previous) = memory.inputs.get_mut(*self.index) {
                *previous = Box::new(value.clone());
            } else {
                memory.inputs.push(Box::new(value.clone()));
            }
        }
        *self.index += 1;
    }

    /// 借入模型或切片，只在值变化时取得所有权，避免每拍复制大列表。
    pub(crate) fn borrowed<T>(&mut self, value: &T)
    where
        T: ToOwned + PartialEq + ?Sized + 'static,
        T::Owned: 'static,
    {
        use std::borrow::Borrow;
        let memory = &mut self.memory;
        let same = memory.inputs.get(*self.index).is_some_and(|previous| {
            previous
                .downcast_ref::<T::Owned>()
                .is_some_and(|saved| saved.borrow() == value)
        });
        if !same {
            memory.valid = false;
            let owned = Box::new(value.to_owned());
            if let Some(previous) = memory.inputs.get_mut(*self.index) {
                *previous = owned;
            } else {
                memory.inputs.push(owned);
            }
        }
        *self.index += 1;
    }

    /// 可选共享模型的缺失、出现与内容变化都属于绘制输入。
    pub(crate) fn optional<T: Clone + PartialEq + 'static>(&mut self, value: Option<&T>) {
        self.observe(&value.is_some());
        if let Some(value) = value {
            self.observe(value);
        }
    }

    /// 时间只应在组件确实按时钟采样显示时声明，而不是自动附到每个组件。
    pub(crate) fn time(&mut self, now: std::time::Instant) {
        self.observe(&now);
    }
}

impl PaintInputs<'_> {
    /// 判断声明是否与最近绘制一致；未声明完的旧依赖不会被忽略。
    fn unchanged(&self) -> bool {
        self.memory.valid && self.memory.inputs.len() == self.index
    }

    /// 变化时直接绘制，不为持续变化的内容复制缓存。输入及下层都稳定后才保存结果。
    fn paint(
        mut self,
        frame: &mut Frame<'_>,
        changed: bool,
        damage: &Damage,
        draw: impl FnOnce(&mut Frame<'_>),
    ) -> bool {
        let memory = &mut self.memory;
        memory.inputs.truncate(self.index);
        if !changed
            && memory.reusable
            && let Some(output) = &memory.output
        {
            copy_damaged(frame.buffer_mut(), output, damage);
            return false;
        }
        memory.reusable = false;
        draw(frame);
        if !changed {
            snapshot_region(&mut memory.output, frame.buffer_mut(), self.area);
            memory.reusable = true;
        }
        memory.valid = true;
        true
    }
}

impl Drop for PaintInputs<'_> {
    fn drop(&mut self) {
        self.cache
            .memory
            .set(Some(std::mem::take(&mut self.memory)));
    }
}

/// 一个组件在本帧的位置；顺序、增删与移动都会影响合成。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// 与组件生命周期一致的缓存身份和当前区域。
    Component {
        /// 所属组件实例的编号。
        id: ComponentId,

        /// 当前区域。
        area: Rect,
    },

    /// 持续变化的合成；结束时同样需要恢复被覆盖的内容。
    Moving,
}

/// 一次组件绘制；闭包只在本帧内借用视图输入。
struct PaintTask<'a> {
    /// 组件声明的区域和依赖；页面转场没有独立组件缓存，影响整帧。
    inputs: Option<PaintInputs<'a>>,

    /// 绘制只借用视图；是否运行由输入与合成失效共同决定。
    draw: Box<dyn FnOnce(&mut Frame<'_>) + 'a>,
}

/// 本次页面按既有顺序组合的组件，不拥有业务数据。
#[derive(Default)]
pub(crate) struct FramePlan<'a> {
    /// 绘制顺序。
    tasks: Vec<PaintTask<'a>>,

    /// 用于识别组件增删、移动和重排。
    placements: Vec<Placement>,

    /// 至少有一个组件的绘制依赖变化。
    changed: bool,
}

impl<'a> FramePlan<'a> {
    /// 按页面顺序加入已绑定实例的只读视图，统一处理比较和绘制登记。
    pub(crate) fn add<V: crate::components::lifecycle::PaintView + 'a>(
        &mut self,
        area: Rect,
        view: crate::components::lifecycle::ComponentView<'a, V>,
    ) {
        view.add_to(area, self);
    }

    /// 添加组件；绘制读取的状态须全部声明，cell 读写须限于声明区域。
    /// 下层变化由合成顺序传播；空区域没有可显示内容，不进入合成。
    pub(crate) fn component(
        &mut self,
        inputs: PaintInputs<'a>,
        draw: impl FnOnce(&mut Frame<'_>) + 'a,
    ) {
        if inputs.area.is_empty() {
            return;
        }
        let changed = !inputs.unchanged();
        self.changed |= changed;
        self.placements.push(Placement::Component {
            id: inputs.cache.id,
            area: inputs.area,
        });
        self.tasks.push(PaintTask {
            inputs: Some(inputs),
            draw: Box::new(draw),
        });
    }

    /// 页面转场等持续变化的合成，仍执行原来的只读绘制。
    pub(crate) fn moving(&mut self, draw: impl FnOnce(&mut Frame<'_>) + 'a) {
        self.changed = true;
        self.placements.push(Placement::Moving);
        self.tasks.push(PaintTask {
            inputs: None,
            draw: Box::new(draw),
        });
    }

    /// 输入和组件排列都没变时，可以省略整个终端帧。
    pub(crate) fn changed_since(&self, previous: &[Placement]) -> bool {
        self.changed || self.placements != previous
    }

    /// 保留当前组件排列，不延长其业务状态或图片资源的生命周期。
    pub(crate) fn placements(&self) -> Vec<Placement> {
        self.placements.clone()
    }

    /// 测试的即时帧入口：保持零次、一次和重复绘制共用生产合成规则。
    #[cfg(test)]
    pub(crate) fn paint(self, frame: &mut Frame<'_>, previous: &[Placement]) -> usize {
        self.recompose(frame, previous, true).1
    }

    /// 找出排列变化涉及的旧、新区域；共同前后缀不因别的组件挂载而失效。
    fn structural_damage(&self, previous: &[Placement], screen: Rect) -> (usize, Damage) {
        let prefix = previous
            .iter()
            .zip(&self.placements)
            .take_while(|(old, new)| old == new)
            .count();
        let suffix = previous
            .iter()
            .skip(prefix)
            .rev()
            .zip(self.placements.iter().skip(prefix).rev())
            .take_while(|(old, new)| old == new)
            .count();
        let mut damage = Damage::new(screen);
        for sequence in [previous, self.placements.as_slice()] {
            for placement in sequence
                .iter()
                .skip(prefix)
                .take(sequence.len() - prefix - suffix)
            {
                damage.add(match placement {
                    Placement::Component { area, .. } => *area,
                    Placement::Moving => screen,
                });
            }
        }
        (prefix, damage)
    }

    /// 只重建失效区域。必须重画的组件扩张到自身完整读写范围，缓存命中只复制相交片段。
    pub(super) fn recompose(
        self,
        frame: &mut Frame<'_>,
        previous: &[Placement],
        full: bool,
    ) -> (Damage, usize) {
        let screen = frame.area();
        let (prefix, structural) = self.structural_damage(previous, screen);
        let mut damage = Damage::new(screen);
        for (y, columns) in structural.rows() {
            damage.add(Rect::new(columns.start, y, columns.end - columns.start, 1));
        }
        if full {
            damage.add(screen);
        }
        let mut below = Damage::new(screen);
        let mut tasks = Vec::with_capacity(self.tasks.len());
        for (index, task) in self.tasks.into_iter().enumerate() {
            let (area, own_changed, reusable) =
                task.inputs
                    .as_ref()
                    .map_or((screen, true, false), |inputs| {
                        (
                            inputs.area(),
                            !inputs.unchanged(),
                            inputs.memory.reusable && inputs.memory.output.is_some(),
                        )
                    });
            let changed = own_changed
                || below.intersects(area)
                || (index >= prefix && structural.intersects(area));
            if changed {
                below.add(area);
                damage.add(area);
            }
            tasks.push((task, area, changed, changed || !reusable));
        }
        // 一个组件可能读取整个区域的背景；扩张后也要让新相交的组件获得完整底图。
        loop {
            let mut expanded = false;
            for (_, area, _, paint) in &tasks {
                if *paint && damage.intersects(*area) {
                    expanded |= damage.add(*area);
                }
            }
            if !expanded {
                break;
            }
        }
        clear_damaged(frame.buffer_mut(), &damage);
        let mut painted = 0;
        for (task, area, changed, _) in tasks {
            if !damage.intersects(area) {
                continue;
            }
            painted += if let Some(inputs) = task.inputs {
                usize::from(inputs.paint(frame, changed, &damage, task.draw))
            } else {
                (task.draw)(frame);
                1
            };
        }
        (damage, painted)
    }
}

/// 只清除本次要重建的画布片段，其他 cell 保留上一帧内容。
fn clear_damaged(target: &mut Buffer, damage: &Damage) {
    for (y, columns) in damage.rows() {
        let start = target.index_of(columns.start, y);
        let end = start + usize::from(columns.end - columns.start);
        if let Some(cells) = target.content.get_mut(start..end) {
            for cell in cells {
                cell.reset();
            }
        }
    }
}

/// 稳定组件只恢复失效区域中的底图，不搬运与本次变化无关的结果。
fn copy_damaged(target: &mut Buffer, source: &Buffer, damage: &Damage) {
    let area = target.area.intersection(source.area);
    for (y, columns) in damage.rows() {
        if y < area.y || y >= area.bottom() {
            continue;
        }
        let left = columns.start.max(area.x);
        let right = columns.end.min(area.right());
        if left >= right {
            continue;
        }
        let from = source.index_of(left, y);
        let to = target.index_of(left, y);
        let width = usize::from(right - left);
        if let (Some(source), Some(target)) = (
            source.content.get(from..from + width),
            target.content.get_mut(to..to + width),
        ) {
            target.clone_from_slice(source);
        }
    }
}

/// 复用区域存储，逐行保存完整 cell；不重新计算字符宽度。
fn snapshot_region(saved: &mut Option<Buffer>, source: &Buffer, area: Rect) {
    let area = area.intersection(source.area);
    let snapshot = saved.get_or_insert_with(|| Buffer::empty(area));
    snapshot.resize(area);
    copy_region(snapshot, source);
}

/// 已确定尺寸的结果逐行回填，不调用文本排版或字符宽度计算。
fn copy_region(target: &mut Buffer, source: &Buffer) {
    let area = target.area.intersection(source.area);
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    for y in area.top()..area.bottom() {
        let from = source.index_of(area.x, y);
        let to = target.index_of(area.x, y);
        if let (Some(source), Some(target)) = (
            source.content.get(from..from + width),
            target.content.get_mut(to..to + width),
        ) {
            target.clone_from_slice(source);
        }
    }
}
