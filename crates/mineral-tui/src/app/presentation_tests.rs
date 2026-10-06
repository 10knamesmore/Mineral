//! 同一更新序列下，绘制次数不能影响后续交互、滚动或搜索展开。

use crate::runtime::action::{Action, SelectionMove};
use crate::test_support::app_with_long_library;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::time::{Duration, Instant};

/// 提交一次已准备页面，保留公共帧计划的组件排列供下一次比较。
fn paint_prepared(
    app: &crate::app::App,
    terminal: &mut Terminal<TestBackend>,
    area: Rect,
) -> color_eyre::Result<Vec<crate::render::memo::Placement>> {
    let view = app.frame_view();
    let plan = crate::view::plan(area, &view);
    let placements = plan.placements();
    terminal.draw(|frame| {
        plan.paint(frame, &[]);
    })?;
    Ok(placements)
}

/// 稳定组件不随空 tick 重画；实际输入、共享数据和配置重载都能使它重新提交。
#[test]
fn idle_frames_are_reused_until_input_model_or_config_changes() -> color_eyre::Result<()> {
    use std::sync::Arc;

    let mut app = app_with_long_library(120, 0)?;
    let area = Rect::new(0, 0, 160, 48);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    let start = Instant::now();
    for tick in 0..80 {
        app.state.tick_frame();
        app.prepare_view(area, start + Duration::from_millis(tick * 16), true);
    }
    let previous = paint_prepared(&app, &mut terminal, area)?;
    app.prepare_view(area, start + Duration::from_secs(2), false);
    assert!(!crate::view::plan(area, &app.frame_view()).changed_since(&previous));

    app.dispatch(Action::MoveSelection(SelectionMove::Down(1)));
    app.prepare_view(area, start + Duration::from_secs(2), false);
    assert_eq!(app.state.ui.browse.tracks.scroll().sel(), 1);
    assert!(crate::view::plan(area, &app.frame_view()).changed_since(&previous));
    let previous = paint_prepared(&app, &mut terminal, area)?;

    let playlist = mineral_model::PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
    let Some(entry) = app
        .state
        .models
        .library
        .tracks
        .get_mut(&playlist)
        .and_then(|tracks| tracks.entries.get_mut(1))
    else {
        color_eyre::eyre::bail!("selected fixture entry missing");
    };
    entry.loved = true;
    app.prepare_view(area, start + Duration::from_secs(2), false);
    assert!(crate::view::plan(area, &app.frame_view()).changed_since(&previous));
    let previous = paint_prepared(&app, &mut terminal, area)?;

    let cfg = Arc::new(crate::config::TuiConfig::defaults()?);
    app.apply_config(cfg);
    app.prepare_view(area, start + Duration::from_secs(2), false);
    assert!(crate::view::plan(area, &app.frame_view()).changed_since(&previous));
    Ok(())
}

/// 图片回填会唤醒使用它的组件；后续准备和可见性记账不会制造新的绘制变化。
#[test]
fn image_readiness_invalidates_paint_but_retention_does_not() -> color_eyre::Result<()> {
    use std::sync::Arc;

    let mut app = app_with_long_library(8, 0)?;
    let area = Rect::new(0, 0, 160, 48);
    let now = Instant::now();
    let url = mineral_model::MediaUrl::remote("https://fixture.invalid/selected.png")?;
    let playlist = mineral_model::PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
    let Some(entry) = app
        .state
        .models
        .library
        .tracks
        .get_mut(&playlist)
        .and_then(|tracks| tracks.entries.first_mut())
    else {
        color_eyre::eyre::bail!("selected fixture entry missing");
    };
    entry.data.song.cover_url = Some(url.clone());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    for _ in 0..80 {
        app.state.tick_frame();
        app.prepare_view(area, now, true);
    }
    let previous = paint_prepared(&app, &mut terminal, area)?;
    app.state
        .resources
        .images
        .cache
        .insert_test(&url, Arc::new(image::DynamicImage::new_rgb8(16, 16)));
    app.prepare_view(area, now, false);
    assert!(crate::view::plan(area, &app.frame_view()).changed_since(&previous));
    let previous = paint_prepared(&app, &mut terminal, area)?;
    for _ in 0..4 {
        app.prepare_view(area, now, false);
        assert!(app.state.resources.images.cache.contains_key(&url));
        assert!(!crate::view::plan(area, &app.frame_view()).changed_since(&previous));
    }
    Ok(())
}

/// 更新和绘制明确分离；这条回归同时覆盖零次绘制后清除筛选、重设尺寸和继续导航。
#[test]
fn presentation_state_does_not_depend_on_paint_count() -> color_eyre::Result<()> {
    let start = Instant::now();
    let mut outcomes = Vec::new();
    for paints in [0, 1, 2] {
        let mut app = app_with_long_library(120, 0)?;
        app.state.ui.browse.view.retempo(1);
        app.state.ui.browse.view.tick();
        let area = Rect::new(0, 0, 120, 40);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
        app.prepare_view(area, start, false);
        app.dispatch(Action::MoveSelection(SelectionMove::Last));
        let mut positions = Vec::new();
        for tick in 1..=12 {
            app.state.tick_frame();
            app.prepare_view(area, start + Duration::from_millis(tick * 16), true);
            for _ in 0..paints {
                terminal.draw(|frame| crate::view::draw(frame, &app.frame_view()))?;
            }
            let list = app.state.ui.browse.tracks.scroll();
            positions.push((
                list.scroll_target(),
                list.offset(120, 10),
                list.position(120),
            ));
        }
        app.dispatch(Action::EnterSearch);
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('9'),
            KeyModifiers::NONE,
        )));
        app.prepare_view(area, start + Duration::from_millis(220), false);
        for _ in 0..paints {
            terminal.draw(|frame| crate::view::draw(frame, &app.frame_view()))?;
        }
        app.dispatch(Action::BackOrClearSearch);
        assert!(
            app.state.ui.browse.tracks.expansion().active.is_some(),
            "清除筛选所需的旧视图由准备入口保留"
        );
        let restored = app.state.ui.browse.tracks.scroll().sel();
        let smaller = Rect::new(0, 0, 80, 24);
        app.handle_event(&Event::Resize(smaller.width, smaller.height));
        app.prepare_view(smaller, start + Duration::from_millis(240), false);
        assert!(app.state.ui.browse.tracks.expansion().active.is_none());
        app.dispatch(Action::MoveSelection(SelectionMove::Down(1)));
        app.prepare_view(smaller, start + Duration::from_millis(256), true);
        outcomes.push((
            positions,
            restored,
            app.state.ui.browse.tracks.scroll().sel(),
            app.state.ui.browse.tracks.scroll().scroll_target(),
            app.state.ui.frame_area,
        ));
    }
    let [without_paint, once, twice] = outcomes.as_slice() else {
        color_eyre::eyre::bail!("缺少绘制零次、一次或两次的执行结果");
    };
    assert_eq!(without_paint, once);
    assert_eq!(once, twice);
    Ok(())
}
