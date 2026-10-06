//! 组件包装类型的实例身份、准备执行与绘制复用边界。

use std::cell::Cell;
use std::sync::Arc;

use ratatui::{Frame, Terminal, backend::TestBackend, layout::Rect};

use super::frame::{FrameEnv, PrepareCx};
use super::lifecycle::{Component, PaintView, Prepare};
use crate::image::{ImageEngine, ImageNeeds, ImageRenderPhase};
use crate::render::memo::{Compositor, Dependencies, FramePlan};
use crate::runtime::scroll::list::ScrollMotion;

/// 记录准备和绘制次数，只有业务值参与绘制依赖比较。
#[derive(Clone, Default)]
struct CounterComponent {
    /// 对外显示的业务值。
    value: u8,

    /// 准备入口执行次数，不是显示依赖。
    preparations: usize,

    /// 纯绘制调用探针，不反馈到组件依赖。
    paints: Cell<usize>,
}

impl Prepare for CounterComponent {
    type Input<'a> = u8;

    fn prepare(&mut self, _area: Rect, input: u8, _cx: &mut PrepareCx<'_>) {
        self.preparations += 1;
        self.value = input;
    }
}

impl PaintView for CounterComponent {
    fn dependencies(&self, _area: Rect, _env: FrameEnv<'_>, inputs: &mut Dependencies<'_>) {
        inputs.observe(&self.value);
    }

    fn paint(&self, _frame: &mut Frame<'_>, _area: Rect, _env: FrameEnv<'_>) {
        self.paints.set(self.paints.get() + 1);
    }
}

/// 移动实例保留身份；复制和重建不会继承原缓存，即使前一帧仍保存其位置。
#[test]
fn component_identity_survives_moves_but_not_new_instances() -> color_eyre::Result<()> {
    let cfg = Arc::new(crate::config::TuiConfig::defaults()?);
    let theme = crate::render::theme::Theme::from_config(cfg.theme());
    let env = FrameEnv {
        config: &cfg,
        theme: &theme,
        now: std::time::Instant::now(),
    };
    let area = Rect::new(0, 0, 12, 2);
    let component = Component::new(CounterComponent::default());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    let mut compositor = Compositor::default();
    let mut plan = FramePlan::default();
    plan.add(area, component.bind_view(env, |state| state));
    let previous = plan.placements();
    compositor.render(plan, &mut terminal.get_frame(), false);
    assert_eq!(component.paints.get(), 1);

    let moved = Box::new(component);
    {
        let mut plan = FramePlan::default();
        plan.add(area, moved.bind_view(env, |state| state));
        assert!(!compositor.needs_frame(&plan, area));
    }

    let cloned = moved.as_ref().clone();
    drop(moved);
    let replacement = Component::new(CounterComponent::default());
    let mut recorded_instances = vec![previous];
    for component in [cloned, replacement] {
        let paints = component.paints.get();
        let mut plan = FramePlan::default();
        plan.add(area, component.bind_view(env, |state| state));
        let placements = plan.placements();
        assert!(!recorded_instances.contains(&placements));
        assert!(compositor.needs_frame(&plan, area));
        compositor.render(plan, &mut terminal.get_frame(), false);
        assert_eq!(component.paints.get(), paints + 1);
        recorded_instances.push(placements);
    }
    Ok(())
}

/// 相同区域和相同依赖的组件交换顺序仍然失效；内容相同不能代替实例身份。
#[test]
fn reordering_instances_invalidates_an_otherwise_unchanged_plan() -> color_eyre::Result<()> {
    let cfg = Arc::new(crate::config::TuiConfig::defaults()?);
    let theme = crate::render::theme::Theme::from_config(cfg.theme());
    let env = FrameEnv {
        config: &cfg,
        theme: &theme,
        now: std::time::Instant::now(),
    };
    let area = Rect::new(0, 0, 12, 2);
    let first = Component::new(CounterComponent::default());
    let second = Component::new(CounterComponent::default());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    let mut compositor = Compositor::default();
    for (order, changed) in [
        ([&first, &second], true),
        ([&first, &second], false),
        ([&second, &first], true),
    ] {
        let mut plan = FramePlan::default();
        for component in order {
            plan.add(area, component.bind_view(env, |state| state));
        }
        assert_eq!(compositor.needs_frame(&plan, area), changed);
        if changed {
            compositor.render(plan, &mut terminal.get_frame(), false);
        }
    }
    Ok(())
}

/// 每次准备都执行；跳过绘制不冻结状态，取消变化计划也不能把未绘制结果当作命中。
#[test]
fn preparation_runs_through_cache_hits_and_cancelled_plans() -> color_eyre::Result<()> {
    let cfg = Arc::new(crate::config::TuiConfig::defaults()?);
    let theme = crate::render::theme::Theme::from_config(cfg.theme());
    let env = FrameEnv {
        config: &cfg,
        theme: &theme,
        now: std::time::Instant::now(),
    };
    let images = ImageEngine::disabled(Arc::clone(&cfg));
    let mut cx = PrepareCx {
        frame: env,
        images: ImageNeeds::new(images.ready()),
        motion: ScrollMotion::Frozen,
        image_phase: ImageRenderPhase::Stable,
        advance: false,
    };
    let area = Rect::new(0, 0, 12, 2);
    let mut component = Component::new(CounterComponent::default());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    let mut compositor = Compositor::default();
    for (value, changed, submit) in [
        (1, true, true),
        (1, false, false),
        (2, true, false),
        (2, true, true),
    ] {
        component.prepare(area, value, &mut cx);
        let mut plan = FramePlan::default();
        plan.add(area, component.bind_view(env, |state| state));
        assert_eq!(compositor.needs_frame(&plan, area), changed);
        if submit {
            compositor.render(plan, &mut terminal.get_frame(), false);
        }
    }
    assert_eq!(component.preparations, 4);
    assert_eq!(component.paints.get(), 2);
    Ok(())
}
