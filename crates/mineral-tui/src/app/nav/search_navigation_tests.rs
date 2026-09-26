//! 歌单搜索往返和清除筛选后的实体定位回归。

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mineral_model::{PlaylistId, SourceKind};
use mineral_protocol::QueueContextWire;
use mineral_task::TaskEvent;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::components::layout::browse::sidebar;
use crate::runtime::state::{PlaylistTracks, View};
use crate::test_support::{TestClient, entry_views, playlist_view, song, with_name};

/// 经真实按键入口操作，涵盖输入态与浏览态的路由。
fn press(app: &mut App, key: KeyCode) {
    app.handle_event(&Event::Key(KeyEvent::new(key, KeyModifiers::empty())));
}

/// 输入并提交当前层的本地过滤词。
fn search(app: &mut App, query: &str) {
    press(app, KeyCode::Char('/'));
    for c in query.chars() {
        press(app, KeyCode::Char(c));
    }
    press(app, KeyCode::Enter);
}

/// 构造超过一屏的歌单；偶数项匹配 Set，过滤下标与原始下标不同。
fn long_playlists() -> color_eyre::Result<(App, Arc<TestClient>)> {
    let client = Arc::new(TestClient::default());
    let (mut app, _) = crate::test_support::app_with_playlists_probed()?;
    app.client = client.clone();
    app.state.library.playlists = (0..40)
        .map(|index| {
            let prefix = if index % 2 == 0 { "Set" } else { "Other" };
            playlist_view(
                &format!("p{index:02}"),
                &format!("{prefix} {index:02}"),
                SourceKind::NETEASE,
                30,
            )
        })
        .collect();
    let id = PlaylistId::new(SourceKind::NETEASE, "p24");
    app.state.library.tracks.insert(
        id,
        PlaylistTracks {
            entries: entry_views(
                (0..30)
                    .map(|i| with_name(song(&format!("t{i}")), &format!("Track {i:02}")))
                    .collect(),
            ),
            complete: true,
            next_offset: None,
        },
    );
    search(&mut app, "Set");
    app.state.browse.nav.playlist.place(12, 4);
    Ok((app, client))
}

/// 绘制真实 sidebar，让视口按应用规则更新。
fn frame(app: &App) -> color_eyre::Result<()> {
    let theme = crate::test_support::default_theme()?;
    let mut terminal = Terminal::new(TestBackend::new(80, 12))?;
    terminal.draw(|f| sidebar::draw(f, f.area(), &app.state, &theme))?;
    Ok(())
}

/// 进入曲目列表后父搜索保留；返回时恢复父列表的选择与滚动位置。
#[test]
fn filtered_playlist_round_trip_preserves_parent_search() -> color_eyre::Result<()> {
    let (mut app, _) = long_playlists()?;
    for _ in 0..40 {
        frame(&app)?;
    }
    let position = (
        app.state.browse.nav.playlist.sel(),
        app.state.browse.nav.playlist.scroll_target(),
    );
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(
        app.state.opened_playlist().map(|p| p.data.id.value()),
        Some("p24")
    );
    assert_eq!(app.state.filtered_tracks().len(), 30);
    while !app.state.browse.view.at_max() {
        app.state.browse.view.tick();
    }
    search(&mut app, "Track");
    assert_eq!(app.state.browse.search.playlists.query(), "Set");
    assert_eq!(app.state.browse.search.tracks.query(), "Track");
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(app.state.browse.view, View::Library);
    assert!(app.state.browse.search.tracks.query().is_empty());
    assert_eq!(app.state.browse.search.playlists.query(), "Set");
    press(&mut app, KeyCode::Char('h'));
    while !app.state.browse.view.at_min() {
        app.state.browse.view.tick();
    }
    frame(&app)?;
    assert_eq!(app.state.browse.view, View::Playlists);
    assert_eq!(
        (
            app.state.browse.nav.playlist.sel(),
            app.state.browse.nav.playlist.scroll_target()
        ),
        position
    );
    Ok(())
}

/// h / Esc 与删除最后一个字符，都按歌单身份清除而非跳回列表开头。
#[test]
fn clearing_playlist_filter_preserves_identity_and_screen_row() -> color_eyre::Result<()> {
    for (key, typing) in [
        (KeyCode::Char('h'), false),
        (KeyCode::Esc, false),
        (KeyCode::Esc, true),
        (KeyCode::Backspace, true),
    ] {
        let (mut app, _) = long_playlists()?;
        if key == KeyCode::Backspace {
            app.state.browse.search.playlists.set_query("S");
        }
        for _ in 0..40 {
            frame(&app)?;
        }
        app.state.browse.search.playlists.typing = typing;
        press(&mut app, key);
        assert!(app.state.browse.search.playlists.query().is_empty());
        assert_eq!(
            app.state.browse.search.playlists.typing,
            key == KeyCode::Backspace,
            "删到空仍留在输入态，其余清除动作退出输入"
        );
        assert_eq!(
            app.state.selected_playlist().map(|p| p.data.id.value()),
            Some("p24")
        );
        for _ in 0..40 {
            frame(&app)?;
            assert_eq!(app.state.browse.nav.playlist.sel(), 24);
            assert_eq!(app.state.browse.nav.playlist.scroll_target(), 20);
        }
    }
    Ok(())
}

/// 同一首歌可在歌单中出现两次；清除曲目过滤必须保留选中的那一次。
#[test]
fn clearing_track_filter_preserves_exact_occurrence() -> color_eyre::Result<()> {
    let (mut app, _) = long_playlists()?;
    let id = PlaylistId::new(SourceKind::NETEASE, "p24");
    let needle = with_name(song("duplicate"), "Needle");
    let entries = (0..30)
        .map(|i| {
            if i == 3 || i == 22 {
                needle.clone()
            } else {
                with_name(song(&format!("other{i}")), "Other")
            }
        })
        .collect();
    app.state.library.tracks.insert(
        id,
        PlaylistTracks {
            entries: entry_views(entries),
            complete: true,
            next_offset: None,
        },
    );
    app.state.library.tracks_generation += 1;
    press(&mut app, KeyCode::Char('l'));
    while !app.state.browse.view.at_max() {
        app.state.browse.view.tick();
    }
    search(&mut app, "Needle");
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.state.filtered_tracks().len(), 2);
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        app.state.browse.nav.track.sel(),
        22,
        "不能跳到同曲的第一个 occurrence"
    );
    assert_eq!(app.state.browse.search.playlists.query(), "Set");
    assert_eq!(app.state.browse.view, View::Library);
    Ok(())
}

/// 后台重排歌单不改变正在看的曲目或播放语境，返回后仍选中同一歌单。
#[test]
fn opened_playlist_identity_survives_parent_reordering() -> color_eyre::Result<()> {
    let (mut app, client) = long_playlists()?;
    press(&mut app, KeyCode::Char('l'));
    while !app.state.browse.view.at_max() {
        app.state.browse.view.tick();
    }
    frame(&app)?;
    let playlists = app
        .state
        .library
        .playlists
        .iter()
        .rev()
        .map(|p| p.data.clone())
        .collect();
    app.state.apply(&TaskEvent::LibrarySnapshot { playlists });
    frame(&app)?;
    press(&mut app, KeyCode::Enter);
    let contexts = client
        .queue_contexts
        .lock()
        .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
    assert!(
        matches!(contexts.last(), Some((_, QueueContextWire::Playlist { id, .. })) if id.value() == "p24"),
        "播放语境必须仍是已打开歌单"
    );
    drop(contexts);
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(
        app.state.selected_playlist().map(|p| p.data.id.value()),
        Some("p24")
    );
    assert_eq!(
        app.state.browse.nav.playlist.sel(),
        7,
        "返回在新排序中按身份定位"
    );
    assert_eq!(app.state.browse.search.playlists.query(), "Set");
    assert_eq!(
        app.state.opened_playlist().map(|p| p.data.id.value()),
        Some("p24"),
        "返回动画保留曲目身份"
    );
    Ok(())
}

/// 详情回包新增更高分命中时，进场、驻留与返回阶段都保留父歌单身份。
#[test]
fn deep_search_updates_keep_selection_during_navigation() -> color_eyre::Result<()> {
    for phase in ["entering", "library", "returning"] {
        let (mut app, _) = long_playlists()?;
        let target = PlaylistId::new(SourceKind::NETEASE, "p24");
        app.state.library.tracks.insert(
            target.clone(),
            PlaylistTracks {
                entries: entry_views(vec![with_name(song("spaced"), "a-b-c")]),
                complete: true,
                next_offset: None,
            },
        );
        app.state.library.tracks_generation += 1;
        search(&mut app, "abc");
        assert_eq!(app.state.filtered_playlists().len(), 1);
        press(&mut app, KeyCode::Char('l'));
        if phase != "entering" {
            while !app.state.browse.view.at_max() {
                app.state.browse.view.tick();
            }
        }
        if phase == "returning" {
            press(&mut app, KeyCode::Char('h'));
        }
        for _ in 0..3 {
            app.state.browse.view.tick();
        }
        frame(&app)?;
        let new_match = PlaylistId::new(SourceKind::NETEASE, "p00");
        app.state.apply(&TaskEvent::PlaylistDetailFetched {
            id: new_match.clone(),
            load: mineral_channel_core::PlaylistLoad::Complete,
            detail: Box::new(mineral_channel_core::PlaylistDetail::complete(
                mineral_model::Playlist::builder()
                    .id(new_match)
                    .name("Set 00".to_owned())
                    .entries(mineral_model::PlaylistEntry::enumerate(vec![with_name(
                        song("exact"),
                        "abc",
                    )]))
                    .build(),
            )),
        });
        assert_eq!(
            app.state
                .filtered_playlists()
                .iter()
                .map(|p| p.data.id.value())
                .collect::<Vec<_>>(),
            vec!["p00", "p24"],
            "前置：新命中确实排到了原歌单之前"
        );
        assert_eq!(
            app.state.selected_playlist_in_list().map(|p| &p.data.id),
            Some(&target),
            "{phase} 回包不能换掉父选中项"
        );
        assert_eq!(
            app.state.opened_playlist().map(|p| &p.data.id),
            Some(&target)
        );
        if phase != "returning" {
            press(&mut app, KeyCode::Char('h'));
        }
        while !app.state.browse.view.at_min() {
            app.state.browse.view.tick();
            frame(&app)?;
        }
        assert_eq!(
            app.state.selected_playlist().map(|p| &p.data.id),
            Some(&target)
        );
        assert_eq!(app.state.browse.search.playlists.query(), "abc");
    }
    Ok(())
}

/// 搜索零命中时没有可进入的歌单，保留当前结果页面。
#[test]
fn activating_empty_results_stays_in_playlists() -> color_eyre::Result<()> {
    let (mut app, _) = long_playlists()?;
    search(&mut app, "zzzzzz");
    assert!(app.state.filtered_playlists().is_empty());
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.state.browse.view, View::Playlists);
    assert!(app.state.browse.nav.opened_playlist.is_none());
    Ok(())
}
