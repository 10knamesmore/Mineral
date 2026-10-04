//! 组件实例的缓存所有权、准备入口和只读绘制契约。

use std::ops::{Deref, DerefMut};

use ratatui::{Frame, layout::Rect};

use super::frame::{FrameEnv, PrepareCx};
use crate::render::memo::{Dependencies, FramePlan, PaintCache};

/// 在既有组件树中持有状态与绘制缓存；移动保留身份，克隆创建独立缓存和身份。
#[derive(Clone, Debug)]
pub(crate) struct Component<C> {
    /// 组件自己的交互和准备状态，普通状态操作不需要接触绘制缓存。
    state: C,

    /// 与此实例一起创建和释放的绘制身份、依赖及结果。
    cache: PaintCache,
}

impl<C> Component<C> {
    /// 挂载已有状态，为这次组件实例分配独立的绘制身份。
    pub(crate) fn new(state: C) -> Self {
        Self {
            state,
            cache: PaintCache::default(),
        }
    }

    /// 把组件的只读视图绑定到所属实例；配置身份与区域比较由公共接线处理。
    pub(crate) fn bind_view<'a, V>(
        &'a self,
        env: FrameEnv<'a>,
        view: impl FnOnce(&'a C) -> V,
    ) -> ComponentView<'a, V> {
        ComponentView {
            view: view(&self.state),
            cache: &self.cache,
            env,
        }
    }
}

impl<C: Prepare> Component<C> {
    /// 按调用者选定的布局准备状态与资源需求，不受绘制缓存是否命中影响。
    pub(crate) fn prepare(&mut self, area: Rect, input: C::Input<'_>, cx: &mut PrepareCx<'_>) {
        self.state.prepare(area, input, cx);
    }
}

impl<C: Default> Default for Component<C> {
    fn default() -> Self {
        Self::new(C::default())
    }
}

impl<C> Deref for Component<C> {
    type Target = C;

    fn deref(&self) -> &C {
        &self.state
    }
}

impl<C> DerefMut for Component<C> {
    fn deref_mut(&mut self) -> &mut C {
        &mut self.state
    }
}

/// 组件准备只修改自身状态，通过上下文声明资源需求；每次调用都执行。
pub(crate) trait Prepare {
    /// 本类组件需要的模型借用与布局选项，不包含全局应用状态。
    type Input<'a>;

    /// 根据明确布局更新状态；转场可以用不同布局多次调用，推进时机由上下文决定。
    fn prepare(&mut self, area: Rect, input: Self::Input<'_>, cx: &mut PrepareCx<'_>);
}

/// 只读绘制输入；依赖声明与绘制都不负责缓存所有权、资源调度或终端提交。
pub(crate) trait PaintView {
    /// 声明实际读取的模型、资源就绪和动画值；区域、配置身份、主题由框架统一观察。
    fn dependencies(&self, area: Rect, env: FrameEnv<'_>, inputs: &mut Dependencies<'_>);

    /// 读写必须限于声明区域；允许省略或重复执行，不推进组件状态。
    fn paint(&self, frame: &mut Frame<'_>, area: Rect, env: FrameEnv<'_>);
}

impl<V: PaintView + ?Sized> PaintView for &V {
    fn dependencies(&self, area: Rect, env: FrameEnv<'_>, inputs: &mut Dependencies<'_>) {
        V::dependencies(self, area, env, inputs);
    }

    fn paint(&self, frame: &mut Frame<'_>, area: Rect, env: FrameEnv<'_>) {
        V::paint(self, frame, area, env);
    }
}

/// 本帧只读视图及其所属实例的缓存借用；不能在视图存活时修改组件状态。
pub(crate) struct ComponentView<'a, V> {
    /// 组件提供的绘制输入，不包含缓存操作能力。
    view: V,

    /// 绑定实例的缓存，不改变原组件生命周期。
    cache: &'a PaintCache,

    /// 本帧配置、主题与采样时间。
    env: FrameEnv<'a>,
}

impl<V> ComponentView<'_, V> {
    /// 借出同一实例的视图，供页面在既有绘制顺序中登记。
    pub(crate) fn as_ref(&self) -> ComponentView<'_, &V> {
        ComponentView {
            view: &self.view,
            cache: self.cache,
            env: self.env,
        }
    }
}

impl<'a, V> ComponentView<'a, V> {
    /// 增补本次布局的绘制选项，仍沿用原组件身份与缓存边界。
    pub(crate) fn map<U>(self, map: impl FnOnce(V) -> U) -> ComponentView<'a, U> {
        ComponentView {
            view: map(self.view),
            cache: self.cache,
            env: self.env,
        }
    }
}

impl<'a, V: PaintView + 'a> ComponentView<'a, V> {
    /// 公共登记入口：比较公共及组件依赖，把纯绘制闭包交给合成器。
    pub(crate) fn add_to(self, area: Rect, plan: &mut FramePlan<'a>) {
        let Self { view, cache, env } = self;
        let mut inputs = cache.inputs(area, env);
        view.dependencies(area, env, &mut inputs.dependencies());
        plan.component(inputs, move |frame| view.paint(frame, area, env));
    }
}

impl<V> Deref for ComponentView<'_, V> {
    type Target = V;

    fn deref(&self) -> &V {
        &self.view
    }
}
