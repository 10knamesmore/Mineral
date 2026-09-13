//! 页面形变的集成回归：真实帧中的文字交接、持续播放信息与浏览位置。

use color_eyre::eyre::eyre;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::app::App;
use crate::components::layout::shared::compute::{compute, compute_search};
use crate::components::layout::shared::transform::morph_search;
use crate::render::anim::Toggle;
use crate::test_support::{
    app_in_fullscreen, app_in_search_morph, app_with_long_library, app_with_queue,
};

/// 通过主帧入口绘制原点为零的终端，保留完整 cell 样式供比较。
fn render(app: &App, area: Rect) -> color_eyre::Result<Buffer> {
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    terminal.draw(|frame| crate::view::draw(frame, app))?;
    Ok(terminal.backend().buffer().clone())
}

/// 查找短 ASCII 文本的屏幕位置；按 cell 扫描，不把 UTF-8 字节偏移当作列号。
fn text_positions(buffer: &Buffer, area: Rect, text: &str) -> Vec<Position> {
    let mut positions = Vec::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let candidate = (x..area.right())
                .take(text.len())
                .filter_map(|column| buffer.cell((column, y)))
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            if candidate == text {
                positions.push(Position::new(x, y));
            }
        }
    }
    positions
}

/// 中途反向只改变目标；前后两段的整帧字形、前景、背景和修饰都不能跳变。
#[test]
fn reversing_page_morph_preserves_the_rendered_frame() -> color_eyre::Result<()> {
    let area = Rect::new(0, 0, 120, 40);
    let mut search = app_in_search_morph(true, true)?;
    search.state.browse.view.retempo(1);
    search.state.browse.view.tick();
    assert!(
        search.state.browse.view.at_max(),
        "前置：浏览端已停在曲目页"
    );

    for (page, mut app, fullscreen) in [
        ("search", search, false),
        ("fullscreen", app_in_fullscreen()?, true),
    ] {
        for ticks in [2, 6] {
            let active = if fullscreen {
                &mut app.state.browse.fullscreen
            } else {
                &mut app.state.channel_search.active
            };
            *active = Toggle::new(8);
            active.set(true);
            for _ in 0..ticks {
                active.tick();
            }
            assert!(!active.settled(), "前置：页面形变尚未落定");
            let entering = render(&app, area)?;

            if fullscreen {
                app.state.browse.fullscreen.set(false);
            } else {
                app.state.channel_search.active.set(false);
            }
            assert_eq!(
                render(&app, area)?,
                entering,
                "{page} 第 {ticks} 拍改为退出但不推进时，画面不得跳变"
            );

            if fullscreen {
                app.state.browse.fullscreen.set(true);
            } else {
                app.state.channel_search.active.set(true);
            }
            assert_eq!(
                render(&app, area)?,
                entering,
                "{page} 同一位置再次改为进入，画面仍应相同"
            );
        }
    }
    Ok(())
}

/// 搜索尚未落定时，两列目标标题已经可见，并处于背景与稳态文字色之间。
#[test]
fn search_target_titles_fade_in_before_the_endpoint() -> color_eyre::Result<()> {
    let area = Rect::new(0, 0, 120, 40);
    let mut app = app_in_search_morph(false, false)?;
    app.state.browse.view.retempo(1);
    app.state.browse.view.tick();
    assert!(app.state.browse.view.at_max(), "前置：浏览端已停在曲目页");
    for _ in 0..2 {
        app.state.channel_search.active.tick();
    }
    assert!(
        !app.state.channel_search.active.at_max(),
        "前置：搜索尚未落定"
    );

    let layout = app.state.cfg.tui().layout();
    let normal = compute(area, layout);
    let search = compute_search(area, layout);
    let current = morph_search(
        &normal,
        &search,
        app.state.channel_search.active.eased_in_out(),
    );
    let fading = render(&app, area)?;
    for _ in 0..2 {
        app.state.channel_search.active.tick();
    }
    assert!(
        app.state.channel_search.active.at_max(),
        "前置：对照帧为搜索稳态"
    );
    let steady = render(&app, area)?;

    for (title, during, endpoint) in [
        ("results", current.left, search.left),
        (
            "Aurora",
            current.right.ok_or_else(|| eyre!("形变缺少详情区域"))?,
            search.right.ok_or_else(|| eyre!("搜索缺少详情区域"))?,
        ),
    ] {
        let caption = |panel: Rect| Rect::new(panel.x, panel.y, panel.width, 1);
        let during_positions = text_positions(&fading, caption(during), title);
        let steady_positions = text_positions(&steady, caption(endpoint), title);
        assert_eq!(during_positions.len(), 1, "{title} 在落定前应完整出现一次");
        assert_eq!(steady_positions.len(), 1, "{title} 的稳态标题应可见");
        let during_cell = during_positions
            .first()
            .and_then(|position| fading.cell(*position))
            .ok_or_else(|| eyre!("缺少形变标题 {title}"))?;
        let steady_cell = steady_positions
            .first()
            .and_then(|position| steady.cell(*position))
            .ok_or_else(|| eyre!("缺少稳态标题 {title}"))?;
        assert_ne!(
            during_cell.fg, during_cell.bg,
            "{title} 已经淡入，不能仍然不可见"
        );
        assert_ne!(
            during_cell.fg, steady_cell.fg,
            "{title} 尚在淡入，不能直接使用稳态色"
        );
    }
    Ok(())
}

/// 两种页面形变中，播放标题持续移动且只画一次，不随页面正文一起淡走。
#[test]
fn playback_title_stays_visible_once_through_page_morphs() -> color_eyre::Result<()> {
    let area = Rect::new(0, 0, 120, 40);
    for (page, fullscreen) in [("search", false), ("fullscreen", true)] {
        let mut app = app_with_queue(3, 0)?;
        let title = app
            .state
            .playback
            .track
            .as_ref()
            .ok_or_else(|| eyre!("缺少在播曲"))?
            .name
            .clone();
        let active = if fullscreen {
            &mut app.state.browse.fullscreen
        } else {
            &mut app.state.channel_search.active
        };
        *active = Toggle::new(8);
        active.set(true);
        let mut positions = Vec::new();
        for tick in 1..8 {
            if fullscreen {
                app.state.browse.fullscreen.tick();
            } else {
                app.state.channel_search.active.tick();
            }
            let frame = render(&app, area)?;
            let matches = text_positions(&frame, area, &title);
            assert_eq!(
                matches.len(),
                1,
                "{page} 第 {tick} 拍应只有一份完整播放标题"
            );
            let position = matches
                .first()
                .copied()
                .ok_or_else(|| eyre!("形变中播放标题消失"))?;
            let cell = frame.cell(position).ok_or_else(|| eyre!("播放标题越界"))?;
            assert_eq!(cell.fg, app.theme.text, "播放标题不应跟随页面文字淡化");
            assert_ne!(cell.fg, cell.bg, "播放标题应持续可见");
            positions.push(position);
        }
        assert!(
            positions.windows(2).any(|pair| pair.first() != pair.last()),
            "{page} 播放标题应随播放栏移动"
        );
    }
    Ok(())
}

/// 列表停在尾部时，搜索的较高视口不能钳改浏览滚动目标；往返后仍在原屏幕行。
#[test]
fn search_round_trip_preserves_the_browse_scroll_position() -> color_eyre::Result<()> {
    let area = Rect::new(0, 0, 120, 40);
    let mut app = app_with_long_library(80, 79)?;
    app.state.browse.view.retempo(1);
    app.state.browse.view.tick();
    assert!(app.state.browse.view.at_max(), "前置：浏览端已停在曲目页");
    for _ in 0..40 {
        render(&app, area)?;
    }
    let normal = compute(area, app.state.cfg.tui().layout());
    let before = render(&app, area)?;
    let selected_before = text_positions(&before, normal.left, "Track 79");
    let scroll_before = app.state.browse.nav.track.scroll_target();
    assert!(scroll_before > 0, "前置：列表已滚到深处");
    assert_eq!(selected_before.len(), 1, "前置：尾部选中曲完整可见");

    app.state.channel_search.active = Toggle::new(8);
    app.state.channel_search.enter(&app.state.caps);
    for entering in [true, false] {
        app.state.channel_search.active.set(entering);
        for tick in 1..=8 {
            app.state.channel_search.active.tick();
            render(&app, area)?;
            assert_eq!(
                app.state.browse.nav.track.scroll_target(),
                scroll_before,
                "搜索进入={entering} 第 {tick} 拍不能改写浏览滚动目标"
            );
        }
    }
    assert!(
        app.state.channel_search.active.at_min(),
        "前置：已回到浏览端点"
    );
    let after = render(&app, area)?;
    assert_eq!(
        text_positions(&after, normal.left, "Track 79"),
        selected_before,
        "搜索往返后，选中曲应回到原来的屏幕行"
    );
    Ok(())
}
