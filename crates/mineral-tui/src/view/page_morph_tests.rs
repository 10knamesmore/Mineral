//! 页面绘制期间的播放反馈和浏览滚动状态回归。

use crate::app::App;
use crate::render::anim::Toggle;
use crate::test_support::{app_with_long_library, app_with_queue};
use ratatui::layout::Rect;

/// 通过主帧入口触发真实绘制。
fn render(app: &mut App, area: Rect) -> color_eyre::Result<()> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))?;
    terminal.draw(|frame| crate::test_support::prepare_and_draw(frame, app))?;
    Ok(())
}

/// 反馈只由 tick 推进，重复绘制、临时尺寸及页面离屏端点不重播或清空它。
#[test]
fn transport_feedback_survives_repeated_paint_resize_and_morph() -> color_eyre::Result<()> {
    use crate::runtime::action::{Action, VolumeDelta};
    let area = Rect::new(0, 0, 120, 40);
    let mut app = app_with_queue(3, 0)?;
    let now = std::time::Instant::now();
    let anim = app.state.cfg.tui().animation();
    app.state.ui.transport.on_action(
        Action::CyclePlayMode,
        app.state.models.playback.mode,
        anim,
        now,
    );
    app.state.ui.transport.on_action(
        Action::NudgeVolume(VolumeDelta(5)),
        app.state.models.playback.mode,
        anim,
        now,
    );
    for _ in 0..5 {
        app.state.tick_frame();
    }
    let before_phase = app.state.ui.transport.controls_opacity();
    assert!(before_phase > 0 && before_phase < 1000);
    render(&mut app, area)?;
    for size in [area, Rect::new(0, 0, 40, 12), Rect::new(0, 0, 100, 30)] {
        render(&mut app, size)?;
        for fullscreen in [true, false] {
            let active = if fullscreen {
                &mut app.state.ui.browse.fullscreen
            } else {
                &mut app.state.ui.channel_search.active
            };
            *active = Toggle::new(8);
            active.set(true);
            for _ in 0..8 {
                if fullscreen {
                    app.state.ui.browse.fullscreen.tick();
                } else {
                    app.state.ui.channel_search.active.tick();
                }
                render(&mut app, size)?;
                assert_eq!(app.state.ui.transport.controls_opacity(), before_phase);
            }
            if fullscreen {
                app.state.ui.browse.fullscreen = Toggle::new(8);
            } else {
                app.state.ui.channel_search.active = Toggle::new(8);
            }
        }
    }
    render(&mut app, area)?;
    assert_eq!(app.state.ui.transport.controls_opacity(), before_phase);
    Ok(())
}

/// 列表停在尾部时，搜索的较高视口不能钳改浏览滚动目标。
#[test]
fn search_round_trip_preserves_the_browse_scroll_position() -> color_eyre::Result<()> {
    let area = Rect::new(0, 0, 120, 40);
    let mut app = app_with_long_library(80, 79)?;
    app.state.ui.browse.view.retempo(1);
    app.state.ui.browse.view.tick();
    assert!(
        app.state.ui.browse.view.at_max(),
        "前置：浏览端已停在曲目页"
    );
    for _ in 0..40 {
        render(&mut app, area)?;
    }
    let scroll_before = app.state.ui.browse.tracks.scroll().scroll_target();
    assert!(scroll_before > 0, "前置：列表已滚到深处");

    app.state.ui.channel_search.active = Toggle::new(8);
    app.state.ui.channel_search.enter(&app.state.models.caps);
    for entering in [true, false] {
        app.state.ui.channel_search.active.set(entering);
        for tick in 1..=8 {
            app.state.ui.channel_search.active.tick();
            render(&mut app, area)?;
            assert_eq!(
                app.state.ui.browse.tracks.scroll().scroll_target(),
                scroll_before,
                "搜索进入={entering} 第 {tick} 拍不能改写浏览滚动目标"
            );
        }
    }
    assert!(
        app.state.ui.channel_search.active.at_min(),
        "前置：已回到浏览端点"
    );
    render(&mut app, area)?;
    assert_eq!(
        app.state.ui.browse.tracks.scroll().scroll_target(),
        scroll_before
    );
    Ok(())
}
