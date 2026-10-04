//! 同一更新序列下，绘制次数不能影响后续交互、滚动或搜索展开。

use crate::runtime::action::{Action, SelectionMove};
use crate::test_support::app_with_long_library;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::time::{Duration, Instant};

/// 更新和绘制明确分离；这条回归同时覆盖零次绘制后清除筛选、重设尺寸和继续导航。
#[test]
fn presentation_state_does_not_depend_on_paint_count() -> color_eyre::Result<()> {
    let start = Instant::now();
    let mut outcomes = Vec::new();
    for paints in [0, 1, 2] {
        let mut app = app_with_long_library(120, 0)?;
        app.state.browse.view.retempo(1);
        app.state.browse.view.tick();
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
            let list = &app.state.browse.nav.track;
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
            app.state.browse.list_expansion.active.is_some(),
            "清除筛选所需的旧视图由准备入口保留"
        );
        let restored = app.state.browse.nav.track.sel();
        let smaller = Rect::new(0, 0, 80, 24);
        app.handle_event(&Event::Resize(smaller.width, smaller.height));
        app.prepare_view(smaller, start + Duration::from_millis(240), false);
        assert!(app.state.browse.list_expansion.active.is_none());
        app.dispatch(Action::MoveSelection(SelectionMove::Down(1)));
        app.prepare_view(smaller, start + Duration::from_millis(256), true);
        outcomes.push((
            positions,
            restored,
            app.state.browse.nav.track.sel(),
            app.state.browse.nav.track.scroll_target(),
            app.state.frame_area,
        ));
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(outcomes[1], outcomes[2]);
    Ok(())
}
