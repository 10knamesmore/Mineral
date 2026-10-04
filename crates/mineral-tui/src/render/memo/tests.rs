//! 缓存只省略计算，不省略输入变化、下层变化和组件卸载的提交。

use std::cell::Cell;
use std::sync::Arc;

use ratatui::{Terminal, backend::TestBackend, layout::Rect, widgets::Paragraph};

use super::damage::Damage;
use super::surface::Surface;
use super::{Compositor, FramePlan, PaintCache};
use crate::components::frame::FrameEnv;

/// 值相同不重画，依赖变化和取消过的计划仍须重画。
#[test]
fn values_are_reused_only_after_a_completed_paint() -> color_eyre::Result<()> {
    let cfg = Arc::new(mineral_config::Config::defaults()?);
    let theme = crate::render::theme::Theme::from_config(cfg.tui().theme());
    let env = FrameEnv {
        config: &cfg,
        theme: &theme,
        now: std::time::Instant::now(),
    };
    let area = Rect::new(0, 0, 12, 2);
    let cache = PaintCache::default();
    let calls = Cell::new(0);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    let mut previous = Vec::new();
    for (value, submit) in [(1, true), (1, false), (2, true)] {
        let mut inputs = cache.inputs(area, env);
        inputs.dependencies().observe(&value);
        let mut plan = FramePlan::default();
        plan.component(inputs, |frame| {
            calls.set(calls.get() + 1);
            frame.render_widget(Paragraph::new("中🙂"), area);
        });
        assert_eq!(plan.changed_since(&previous), submit);
        if submit {
            let placements = plan.placements();
            terminal.draw(|frame| {
                plan.paint(frame, &previous);
            })?;
            previous = placements;
        }
    }
    assert_eq!(calls.get(), 2);
    let mut cancelled = cache.inputs(area, env);
    cancelled.dependencies().observe(&3);
    drop(cancelled);
    let mut inputs = cache.inputs(area, env);
    inputs.dependencies().observe(&3);
    let mut retry = FramePlan::default();
    retry.component(inputs, |_| calls.set(calls.get() + 1));
    assert!(retry.changed_since(&previous));
    terminal.draw(|frame| {
        retry.paint(frame, &previous);
    })?;
    assert_eq!(calls.get(), 3);
    Ok(())
}

/// 已命中的上层缓存也必须响应下层内容变化，以及下层组件的卸载。
#[test]
fn underlying_changes_and_unmounts_are_not_hidden_by_cache_hits() -> color_eyre::Result<()> {
    let cfg = Arc::new(mineral_config::Config::defaults()?);
    let theme = crate::render::theme::Theme::from_config(cfg.tui().theme());
    let env = FrameEnv {
        config: &cfg,
        theme: &theme,
        now: std::time::Instant::now(),
    };
    let area = Rect::new(0, 0, 12, 2);
    let background = PaintCache::default();
    let child = PaintCache::default();
    let child_calls = Cell::new(0);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    let mut previous = Vec::new();
    let mut previous_background = None;
    let mut frames = 0;
    for value in std::iter::repeat_n(Some(1), 8)
        .chain(std::iter::repeat_n(Some(2), 8))
        .chain([None])
    {
        let before = child_calls.get();
        let changed = previous.is_empty() || value != previous_background;
        let mut plan = FramePlan::default();
        if let Some(value) = value {
            let mut base = background.inputs(area, env);
            base.dependencies().observe(&value);
            plan.component(base, move |frame| {
                frame.render_widget(Paragraph::new(value.to_string()), area)
            });
        }
        plan.component(child.inputs(area, env), |_| {
            child_calls.set(child_calls.get() + 1)
        });
        assert_eq!(plan.changed_since(&previous), changed);
        let placements = plan.placements();
        terminal.draw(|frame| {
            plan.paint(frame, &previous);
        })?;
        if changed {
            assert!(child_calls.get() > before);
        }
        previous = placements;
        previous_background = value;
        frames += 1;
    }
    assert!(child_calls.get() < frames);
    Ok(())
}

/// 前景连续变化时，稳定的下层仍能复用，不必随整帧重新运行绘制。
#[test]
fn a_changing_component_does_not_repaint_stable_layers_forever() -> color_eyre::Result<()> {
    let cfg = Arc::new(mineral_config::Config::defaults()?);
    let theme = crate::render::theme::Theme::from_config(cfg.tui().theme());
    let env = FrameEnv {
        config: &cfg,
        theme: &theme,
        now: std::time::Instant::now(),
    };
    let area = Rect::new(0, 0, 12, 2);
    let base = PaintCache::default();
    let foreground = PaintCache::default();
    let base_calls = Cell::new(0);
    let foreground_calls = Cell::new(0);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    let mut previous = Vec::new();
    for value in 0..8 {
        let mut plan = FramePlan::default();
        plan.component(base.inputs(area, env), |frame| {
            base_calls.set(base_calls.get() + 1);
            frame.render_widget(Paragraph::new("中🙂"), area);
        });
        let mut inputs = foreground.inputs(Rect::new(0, 0, 1, 1), env);
        inputs.dependencies().observe(&value);
        plan.component(inputs, |frame| {
            foreground_calls.set(foreground_calls.get() + 1);
            frame.render_widget(Paragraph::new(value.to_string()), Rect::new(0, 0, 1, 1));
        });
        assert!(plan.changed_since(&previous));
        let placements = plan.placements();
        terminal.draw(|frame| {
            plan.paint(frame, &previous);
        })?;
        previous = placements;
    }
    assert_eq!(foreground_calls.get(), 8);
    assert!(base_calls.get() > 0 && base_calls.get() < foreground_calls.get());
    Ok(())
}

/// 局部更新不得清空旁边的保留画布，也不提交或重画不相交的组件。
#[test]
fn retained_frame_leaves_unaffected_components_and_cells_alone() -> color_eyre::Result<()> {
    let cfg = Arc::new(mineral_config::Config::defaults()?);
    let theme = crate::render::theme::Theme::from_config(cfg.tui().theme());
    let env = FrameEnv {
        config: &cfg,
        theme: &theme,
        now: std::time::Instant::now(),
    };
    let area = Rect::new(0, 0, 16, 4);
    let left_area = Rect::new(0, 0, 4, 4);
    let right_area = Rect::new(8, 0, 8, 4);
    let base = PaintCache::default();
    let left = PaintCache::default();
    let right = PaintCache::default();
    let right_calls = Cell::new(0);
    let mut terminal = Terminal::new(TestBackend::new(16, 4))?;
    let mut compositor = Compositor::default();
    for value in 0..4 {
        let before_calls = right_calls.get();
        let before_cell = terminal.current_buffer_mut().cell((10, 1)).cloned();
        let mut plan = FramePlan::default();
        plan.component(base.inputs(area, env), |_| {});
        let mut inputs = left.inputs(left_area, env);
        inputs.dependencies().observe(&value);
        plan.component(inputs, |frame| {
            frame.render_widget(Paragraph::new(value.to_string()), left_area);
        });
        plan.component(right.inputs(right_area, env), |frame| {
            right_calls.set(right_calls.get() + 1);
            frame
                .buffer_mut()
                .set_string(10, 1, "界", ratatui::style::Style::default());
        });
        let update = compositor.render(plan, &mut terminal.get_frame(), false);
        if value >= 2 {
            assert_eq!(right_calls.get(), before_calls);
            assert_eq!(
                terminal.current_buffer_mut().cell((10, 1)),
                before_cell.as_ref()
            );
            assert!(
                compositor
                    .updates(&update)
                    .all(|(x, y, _)| left_area.contains((x, y).into()))
            );
        }
    }
    Ok(())
}

/// 移动和卸载只恢复旧、新位置；下层缓存能局部恢复，而不是再画整个背景。
#[test]
fn retained_frame_restores_moved_and_removed_layers() -> color_eyre::Result<()> {
    let cfg = Arc::new(mineral_config::Config::defaults()?);
    let theme = crate::render::theme::Theme::from_config(cfg.tui().theme());
    let env = FrameEnv {
        config: &cfg,
        theme: &theme,
        now: std::time::Instant::now(),
    };
    let area = Rect::new(0, 0, 16, 4);
    let base = PaintCache::default();
    let overlay = PaintCache::default();
    let base_calls = Cell::new(0);
    let mut terminal = Terminal::new(TestBackend::new(16, 4))?;
    let mut compositor = Compositor::default();
    for (step, x) in [Some(2), Some(4), Some(8), None].into_iter().enumerate() {
        let calls = base_calls.get();
        let mut plan = FramePlan::default();
        plan.component(base.inputs(area, env), |_| {
            base_calls.set(base_calls.get() + 1)
        });
        if let Some(x) = x {
            let position = Rect::new(x, 1, 2, 1);
            plan.component(overlay.inputs(position, env), move |frame| {
                frame.render_widget(Paragraph::new("中"), position);
            });
        }
        let update = compositor.render(plan, &mut terminal.get_frame(), false);
        if step >= 2 {
            assert_eq!(base_calls.get(), calls);
            assert!(!update.is_empty());
            assert!(compositor.updates(&update).all(|(_, y, _)| y == 1));
        }
    }
    Ok(())
}

/// 局部提交保持 ratatui 的宽字符覆盖、skip 和符号变化契约，不固定应用布局。
#[test]
fn regional_diff_preserves_wide_character_and_skip_protocol() {
    use ratatui::buffer::Buffer;
    use ratatui::style::Style;

    let area = Rect::new(0, 0, 12, 2);
    let mut old = Buffer::empty(area);
    old.set_string(2, 0, "中", Style::default());
    old.set_string(6, 0, "🙂", Style::default());
    let mut surface = Surface::new(area);
    let mut all = Damage::new(area);
    all.add(area);
    surface.diff(&old, &all);

    let mut new = old.clone();
    new.set_string(2, 0, "ab", Style::default());
    new.set_string(6, 0, "cd", Style::default());
    if let Some(cell) = new.cell_mut((8, 0)) {
        cell.set_symbol("x").set_skip(true);
    }
    let mut damage = Damage::new(area);
    damage.add(Rect::new(2, 0, 1, 1));
    damage.add(Rect::new(6, 0, 3, 1));
    let actual = surface.diff(&new, &damage);
    let expected = old
        .diff(&new)
        .into_iter()
        .map(|(x, y, cell)| (x, y, cell.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        surface
            .updates(&actual)
            .map(|(x, y, cell)| (x, y, cell.clone()))
            .collect::<Vec<_>>(),
        expected
    );

    let old = new;
    let mut new = old.clone();
    new.set_string(2, 0, "中", Style::default());
    surface.diff(&new, &damage);
    let old = new;
    let mut new = old.clone();
    if let Some(cell) = new.cell_mut((3, 0)) {
        cell.set_symbol("q");
    }
    let mut tail = Damage::new(area);
    tail.add(Rect::new(3, 0, 1, 1));
    let actual = surface.diff(&new, &tail);
    assert_eq!(
        surface
            .updates(&actual)
            .map(|(x, y, cell)| (x, y, cell.clone()))
            .collect::<Vec<_>>(),
        old.diff(&new)
            .into_iter()
            .map(|(x, y, cell)| (x, y, cell.clone()))
            .collect::<Vec<_>>()
    );
}
