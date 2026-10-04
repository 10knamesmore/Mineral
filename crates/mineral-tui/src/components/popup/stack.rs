//! 浮层的挂载、开合动画与遍历；应用通过回调接入具体内容。

use super::component::{Overlay, full_rect};
use crate::render::anim::Transition;
use crate::runtime::state::OverlayReveal;
use ratatui::layout::Rect;

/// 一层内容及其容器托管的开合状态。
struct Mounted<C> {
    /// 调用方提供的内容。
    content: C,

    /// 当前开合动画。
    animation: Transition,
}

/// 完整面板的布局和层间焦点交接，不含业务状态。
#[derive(Clone, Copy)]
pub(crate) struct OverlayLayout {
    /// 不受揭开进度影响的完整矩形。
    pub(crate) area: Rect,

    /// 是否持有键盘焦点。
    pub(crate) focused: bool,

    /// 本层及直接上层的揭开进度。
    pub(crate) reveal: OverlayReveal,
}

/// 由应用选择内容类型的浮层栈。
pub(crate) struct OverlayStack<C> {
    /// 自底向上的挂载层。
    layers: Vec<Mounted<C>>,

    /// 开合时长，热更新时就地调整。
    animation_ticks: u16,
}

impl<C: Overlay> OverlayStack<C> {
    /// 创建尚未挂载内容的栈。
    pub(crate) fn new(animation_ticks: u16) -> Self {
        Self {
            layers: Vec::new(),
            animation_ticks,
        }
    }

    /// 挂载内容并开始开合动画。
    pub(crate) fn push(&mut self, content: C) {
        let ticks = if content.chrome().animated {
            self.animation_ticks
        } else {
            1
        };
        let mut animation = Transition::new(ticks);
        animation.enter();
        self.layers.push(Mounted { content, animation });
    }

    /// 修改开合速度，保留每层当前进度。
    pub(crate) fn retempo(&mut self, ticks: u16) {
        self.animation_ticks = ticks;
        for layer in &mut self.layers {
            layer.animation.retempo(ticks);
        }
    }

    /// 开始关闭栈顶，动画结束后再卸载。
    pub(crate) fn close_top(&mut self) {
        if let Some(layer) = self.layers.last_mut() {
            layer.animation.leave();
        }
    }

    /// 推进容器动画，并让应用更新每层自己的状态。
    pub(crate) fn advance(&mut self, mut update: impl FnMut(&mut C)) {
        for layer in &mut self.layers {
            layer.animation.tick();
            update(&mut layer.content);
        }
        self.layers.retain(|layer| layer.animation.active());
    }

    /// 计算完整布局，组件通过回调准备自己的显示输入。
    pub(crate) fn prepare(
        &mut self,
        area: Rect,
        config: &mineral_config::LayoutConfig,
        dock_right: bool,
        mut visit: impl FnMut(&mut C, OverlayLayout),
    ) {
        let top = self
            .layers
            .iter()
            .rposition(|layer| !layer.animation.leaving());
        let reveals = self
            .layers
            .iter()
            .map(|layer| layer.animation.eased_settle())
            .collect::<Vec<_>>();
        for (index, layer) in self.layers.iter_mut().enumerate() {
            let (area, _) = full_rect(&layer.content.chrome(), area, config, dock_right);
            if area.width < 4 || area.height < 3 {
                continue;
            }
            visit(
                &mut layer.content,
                OverlayLayout {
                    area,
                    focused: Some(index) == top,
                    reveal: OverlayReveal {
                        own: layer.animation.eased_settle(),
                        above: reveals.get(index + 1).copied().unwrap_or(0),
                    },
                },
            );
        }
    }

    /// 自底向上访问显示中的内容，绘制次数不修改栈。
    pub(crate) fn visit(&self, mut paint: impl FnMut(&C, u16, bool)) {
        let top = self
            .layers
            .iter()
            .rposition(|layer| !layer.animation.leaving());
        for (index, layer) in self.layers.iter().enumerate() {
            paint(
                &layer.content,
                layer.animation.eased_settle(),
                Some(index) == top,
            );
        }
    }

    /// 当前挂载数量，包含离场动画尚未完成的层。
    pub(crate) fn len(&self) -> usize {
        self.layers.len()
    }

    /// 是否有正在关闭的居中层，用于终端输出的遮挡恢复。
    pub(crate) fn any_leaving_centered(&self) -> bool {
        self.layers.iter().any(|layer| {
            let chrome = layer.content.chrome();
            layer.animation.leaving() && !chrome.dock && chrome.anchor.is_none()
        })
    }

    /// 借用所有已挂载内容。
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &C> {
        self.layers.iter().map(|layer| &layer.content)
    }

    /// 更新已挂载内容自己的状态。
    pub(crate) fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut C> {
        self.layers.iter_mut().map(|layer| &mut layer.content)
    }

    /// 借用尚未开始离场的内容。
    pub(crate) fn live(&self) -> impl DoubleEndedIterator<Item = &C> {
        self.layers
            .iter()
            .filter(|layer| !layer.animation.leaving())
            .map(|layer| &layer.content)
    }

    /// 更新尚未开始离场的内容。
    pub(crate) fn live_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut C> {
        self.layers
            .iter_mut()
            .filter(|layer| !layer.animation.leaving())
            .map(|layer| &mut layer.content)
    }

    /// 输入由最后一个尚未离场的内容接收。
    pub(crate) fn active(&self) -> Option<&C> {
        self.live().next_back()
    }

    /// 借出当前输入接收者。
    pub(crate) fn active_mut(&mut self) -> Option<&mut C> {
        self.live_mut().next_back()
    }
}
