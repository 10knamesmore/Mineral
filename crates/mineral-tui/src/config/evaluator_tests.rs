//! TUI 配置、独立宿主、复制闭包生命周期与事务效果的边界测试。

use mineral_model::{AlbumId, ArtistId, PlaylistId, SourceKind};
use mineral_protocol::{BusValue, ToastKind};
use mineral_test::song;

use super::evaluator::{Error, evaluate_tui};
use mineral_script::TuiCommand;
use mineral_script::{ConfigOverrideOp, CopyTemplateCtx, SourceWebUrls, WatchdogConfig};

/// 将 API 边界测试放入真实 TUI 配置的 tui.lua 的 setup。
fn setup_source(body: &str) -> String {
    format!("return {{ setup = function(mineral) {body} end }}")
}

/// 本地复制回调的歌曲输入。
fn copy_song() -> CopyTemplateCtx {
    CopyTemplateCtx::Song(Box::new(song("1")))
}

#[test]
fn hosts_are_separate_and_setup_receives_tui_api() -> color_eyre::Result<()> {
    let (lua, _host) = vm_with_host()?;
    lua.load(
        r#"
        assert(_G.mineral == nil)
        local mineral = require("mineral.daemon")
        assert(require("mineral.daemon") == mineral)
        assert(type(mineral.player.play) == "function")
        assert(type(mineral.store.get) == "function")
        assert(type(mineral.store.set) == "function")
        assert(type(mineral.hook) == "function")
        assert(type(mineral.DEFER) == "table")
        assert(type(mineral.config.override) == "function")
        assert(type(mineral.log.info) == "function")
        assert(type(mineral.sys.hostname) == "string")
        assert(mineral.ui == nil and mineral.store.inc == nil)
        assert(not pcall(require, "mineral.tui"))
        for _, key in ipairs({"on", "observe", "get", "action", "bind", "timer", "spawn", "emit", "on_message"}) do
            assert(mineral[key] == nil)
        end
        "#,
    )
    .exec()?;
    let loaded = evaluate_tui(
        &setup_source(
            r#"
            assert(_G.mineral == nil)
            assert(require("mineral.tui") == mineral)
            assert(type(mineral.ui.toast) == "function")
            assert(type(mineral.config.override) == "function")
            assert(type(mineral.log.info) == "function")
            assert(type(mineral.sys.hostname) == "string")
            assert(not pcall(require, "mineral.daemon"))
            for _, key in ipairs({"player", "download", "queue", "library", "store", "hook", "DEFER", "on", "observe", "get", "action", "bind", "timer", "spawn", "emit", "on_message"}) do
                assert(mineral[key] == nil)
            end
            "#,
        ),
        "tui.lua",
        lax_watchdog(),
    )?;
    assert!(loaded.commands.is_empty());
    Ok(())
}

#[test]
fn tui_file_supplies_config_and_setup_effects() -> color_eyre::Result<()> {
    let loaded = evaluate_tui(
        r#"
        return {
            waveform = { enabled = false },
            setup = function(api)
                api.config.override("waveform.enabled", true)
                api.ui.window_title("local title")
            end,
        }
        "#,
        "tui.lua",
        lax_watchdog(),
    )?;
    assert!(!loaded.config.waveform().enabled());
    assert_eq!(
        loaded
            .tree
            .pointer("/waveform/enabled")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert!(matches!(loaded.commands.as_slice(), [
        TuiCommand::ConfigOverride { ops },
        TuiCommand::WindowTitle { text: Some(_) },
    ] if ops == &[ConfigOverrideOp {
        path: "waveform.enabled".to_owned(),
        value: Some(BusValue::Bool(true)),
    }]));
    Ok(())
}

#[test]
fn tui_commands_keep_order_and_parse_notification_options() -> color_eyre::Result<()> {
    let loaded = evaluate_tui(
        &setup_source(
            r##"
            mineral.ui.toast({ "prefix", { "value", fg = "accent", bold = true } },
                { kind = "warn", id = "toast", ttl_secs = 10 })
            mineral.ui.card {
                title = { "title", { "!", fg = "peach" } },
                kind = "error", id = "card", ttl_secs = 8,
                body = { "one\ntwo", { { "three", fg = "#cc6600", align = "right" } } },
            }
            mineral.ui.window_title("window")
            mineral.ui.window_title(nil)
            "##,
        ),
        "tui.lua",
        lax_watchdog(),
    )?;
    assert!(matches!(loaded.commands.as_slice(), [
        TuiCommand::Toast { kind: ToastKind::Warn, id: Some(toast), ttl_secs: Some(10), .. },
        TuiCommand::Card { kind: ToastKind::Error, id: Some(card), ttl_secs: Some(8), .. },
        TuiCommand::WindowTitle { text: Some(_) },
        TuiCommand::WindowTitle { text: None },
    ] if toast == "toast" && card == "card"));
    Ok(())
}

#[test]
fn tui_config_uses_shared_leaf_parser_and_nil_revoke() -> color_eyre::Result<()> {
    let loaded = evaluate_tui(
        &setup_source(
            r#"
            mineral.config.override({ waveform = { enabled = false } })
            mineral.config.override("flags", { "a", "b" })
            mineral.config.override("waveform.enabled", nil)
            mineral.config.override({})
            "#,
        ),
        "tui.lua",
        lax_watchdog(),
    )?;
    assert_eq!(
        loaded.commands,
        vec![
            TuiCommand::ConfigOverride {
                ops: vec![ConfigOverrideOp {
                    path: "waveform.enabled".to_owned(),
                    value: Some(BusValue::Bool(false)),
                }],
            },
            TuiCommand::ConfigOverride {
                ops: vec![ConfigOverrideOp {
                    path: "flags".to_owned(),
                    value: Some(BusValue::Array(vec![
                        BusValue::Str("a".to_owned()),
                        BusValue::Str("b".to_owned()),
                    ])),
                }],
            },
            TuiCommand::ConfigOverride {
                ops: vec![ConfigOverrideOp {
                    path: "waveform.enabled".to_owned(),
                    value: None,
                }],
            },
        ]
    );
    Ok(())
}

#[test]
fn nil_toast_is_skipped_and_scalar_input_is_accepted() -> color_eyre::Result<()> {
    let loaded = evaluate_tui(
        &setup_source(
            r#"
            mineral.ui.toast(nil)
            mineral.ui.toast(42)
            mineral.ui.toast(true)
            mineral.ui.card { body = { "body" } }
            "#,
        ),
        "tui.lua",
        lax_watchdog(),
    )?;
    assert!(matches!(
        loaded.commands.as_slice(),
        [
            TuiCommand::Toast {
                kind: ToastKind::Info,
                id: None,
                ttl_secs: None,
                ..
            },
            TuiCommand::Toast {
                kind: ToastKind::Info,
                id: None,
                ttl_secs: None,
                ..
            },
            TuiCommand::Card {
                kind: ToastKind::Info,
                id: None,
                ttl_secs: None,
                ..
            },
        ]
    ));
    Ok(())
}

#[test]
fn failed_setup_returns_no_partial_commands() -> color_eyre::Result<()> {
    for failure in [
        "error('failed')",
        "mineral.ui.toast('x', { kind = 'fatal' })",
        "mineral.ui.toast('x', { ttl_secs = -1 })",
        "mineral.ui.card { title = 'missing body' }",
        "mineral.ui.card { ttl_secs = -1, body = {'x'} }",
        "mineral.ui.card { title = false, body = {'x'} }",
        "mineral.ui.card { body = {false} }",
        "mineral.ui.card { body = {{{fg = 'red'}}} }",
        "mineral.ui.card { body = {{{'x', fg = 'magenta'}}} }",
        "mineral.ui.card { body = {{{'x', align = 'top'}}} }",
        "mineral.ui.card { kind = 'fatal', body = {'x'} }",
        "mineral.ui.window_title({})",
        "mineral.config.override({ x = function() end })",
    ] {
        let source = setup_source(&format!("mineral.ui.toast('before'); {failure}"));
        assert!(matches!(
            evaluate_tui(&source, "tui.lua", lax_watchdog()),
            Err(Error::Script(mineral_script::Error::Lua { .. }))
        ));
    }
    assert!(matches!(
        evaluate_tui("syntax error", "tui.lua", lax_watchdog()),
        Err(Error::Config(_))
    ));
    assert!(
        evaluate_tui("return {}", "tui.lua", lax_watchdog())?
            .commands
            .is_empty()
    );
    Ok(())
}

#[test]
fn invalid_config_discards_top_level_effects_and_keeps_existing_runtime() -> color_eyre::Result<()>
{
    let old = evaluate_tui(
        r#"return { copy = { templates = {
            { label = "ID", template = function(s) return s.id end },
        } } }"#,
        "tui.lua",
        lax_watchdog(),
    )?;
    let failed = evaluate_tui(
        r#"
        local api = require("mineral.tui")
        api.ui.window_title("uncommitted")
        return { audio = {} }
        "#,
        "tui.lua",
        lax_watchdog(),
    );
    assert!(matches!(failed, Err(Error::Config(_))));
    let (text, commands) = old.runtime.render_copy_template(0, copy_song())?;
    assert_eq!(text, "netease:1");
    assert!(commands.is_empty());
    Ok(())
}

#[test]
fn top_level_and_setup_loops_are_guarded() {
    let watchdog = WatchdogConfig::builder()
        .instruction_interval(1_000)
        .soft_wall(std::time::Duration::from_millis(10))
        .hard_wall(std::time::Duration::from_millis(50))
        .build();
    for source in [
        "local api = require('mineral.tui'); api.ui.toast('before'); while true do end"
            .to_owned(),
        r#"return {
            script = { watchdog_instruction_interval = 1000, watchdog_soft_wall_ms = 10, watchdog_hard_wall_ms = 50 },
            setup = function(api) api.ui.toast('before'); while true do end end,
        }"#.to_owned(),
    ] {
        let started = std::time::Instant::now();
        assert!(evaluate_tui(&source, "tui.lua", watchdog).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}

#[test]
fn copy_closure_state_persists_and_reloading_resets_it() -> color_eyre::Result<()> {
    let source = r#"
        local api
        local count = 0
        return {
            setup = function(host) api = host end,
            copy = { templates = {
                { label = "Count", template = function(s)
                    assert(api.player == nil and not pcall(require, "mineral.daemon"))
                    assert(s.index == nil)
                    count = count + 1
                    api.config.override("waveform.enabled", count % 2 == 0)
                    return tostring(count) .. ":" .. s.id
                end },
            } },
        }
    "#;
    let loaded = evaluate_tui(source, "tui.lua", lax_watchdog())?;
    assert!(loaded.commands.is_empty());
    for (count, enabled) in [(1, false), (2, true)] {
        let (text, commands) = loaded.runtime.render_copy_template(0, copy_song())?;
        assert_eq!(text, format!("{count}:netease:1"));
        assert_eq!(
            commands,
            vec![TuiCommand::ConfigOverride {
                ops: vec![ConfigOverrideOp {
                    path: "waveform.enabled".to_owned(),
                    value: Some(BusValue::Bool(enabled)),
                }],
            }]
        );
    }
    let reloaded = evaluate_tui(source, "tui.lua", lax_watchdog())?;
    let (text, _) = reloaded.runtime.render_copy_template(0, copy_song())?;
    assert_eq!(text, "1:netease:1");
    let (text, _) = loaded.runtime.render_copy_template(0, copy_song())?;
    assert_eq!(text, "3:netease:1");
    Ok(())
}

#[test]
fn failed_copy_discards_effects_without_leaking_into_next_call() -> color_eyre::Result<()> {
    let loaded = evaluate_tui(
        r#"
        local api
        local attempt = 0
        return {
            setup = function(host) api = host end,
            copy = { templates = {
                { label = "Fail once", template = function(s)
                    attempt = attempt + 1
                    if attempt == 1 then
                        api.ui.window_title("uncommitted")
                        api.config.override("waveform.enabled", false)
                        error("failed")
                    end
                    return s.id
                end },
            } },
        }
        "#,
        "tui.lua",
        lax_watchdog(),
    )?;
    assert!(matches!(
        loaded.runtime.render_copy_template(0, copy_song()),
        Err(mineral_script::Error::Lua { .. })
    ));
    let (text, commands) = loaded.runtime.render_copy_template(0, copy_song())?;
    assert_eq!(text, "netease:1");
    assert!(commands.is_empty());
    assert!(matches!(
        loaded.runtime.render_copy_template(9, copy_song()),
        Err(mineral_script::Error::MissingFunction { index: 9, .. })
    ));
    Ok(())
}

#[test]
fn copy_watchdog_interrupts_and_discards_effects() -> color_eyre::Result<()> {
    let watchdog = WatchdogConfig::builder()
        .instruction_interval(1_000)
        .soft_wall(std::time::Duration::from_millis(10))
        .hard_wall(std::time::Duration::from_millis(50))
        .build();
    let loaded = evaluate_tui(
        r#"
        local api
        return {
            script = { watchdog_instruction_interval = 1000, watchdog_soft_wall_ms = 10, watchdog_hard_wall_ms = 50 },
            setup = function(host) api = host end,
            copy = { templates = {
                { label = "Loop", template = function()
                    api.ui.window_title("uncommitted")
                    while true do end
                end },
                { label = "ID", template = function(s) return s.id end },
            } },
        }
        "#,
        "tui.lua",
        watchdog,
    )?;
    let started = std::time::Instant::now();
    assert!(loaded.runtime.render_copy_template(0, copy_song()).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    let (text, commands) = loaded.runtime.render_copy_template(1, copy_song())?;
    assert_eq!(text, "netease:1");
    assert!(commands.is_empty());
    Ok(())
}

#[test]
fn local_copy_projects_all_entities_and_seeded_web_urls() -> color_eyre::Result<()> {
    let loaded = evaluate_tui(
        r#"
        return { copy = { templates = {
            { label = "Song", template = function(s)
                assert(s.id == "netease:1" and s.source == "netease" and s.index == nil)
                assert(s.url == "https://x.example/song/1")
                return s.id
            end },
            { label = "Playlist", context = "playlist", template = function(p)
                assert(p.id == "netease:p1" and p.track_count == 1)
                assert(#p.songs == 1 and p.songs[1].id == "netease:1")
                assert(p.url == "https://x.example/playlist/p1")
                return p.id
            end },
            { label = "Album", context = "album", template = function(a)
                assert(a.id == "netease:a1" and a.track_count == nil)
                assert(#a.songs == 1 and a.songs[1].id == "netease:1")
                assert(a.url == "https://x.example/album/a1")
                return a.id
            end },
            { label = "Artist", context = "artist", template = function(a)
                assert(a.id == "netease:r1" and a.song_count == nil)
                assert(#a.songs == 1 and a.songs[1].id == "netease:1")
                assert(a.url == nil)
                return a.id
            end },
        } } }
        "#,
        "tui.lua",
        lax_watchdog(),
    )?;
    loaded.runtime.seed_web_url_templates([SourceWebUrls {
        source: "netease".to_owned(),
        song: Some("https://x.example/song/{id}".to_owned()),
        playlist: Some("https://x.example/playlist/{id}".to_owned()),
        album: Some("https://x.example/album/{id}".to_owned()),
        artist: None,
    }])?;
    let playlist = mineral_model::Playlist::builder()
        .id(PlaylistId::new(SourceKind::NETEASE, "p1"))
        .name("list".to_owned())
        .track_count(1)
        .entries(mineral_model::PlaylistEntry::enumerate(vec![song("1")]))
        .build();
    let album = mineral_model::Album::builder()
        .id(AlbumId::new(SourceKind::NETEASE, "a1"))
        .name("album".to_owned())
        .tracks(mineral_model::AlbumTrack::enumerate(vec![song("1")]))
        .build();
    let artist = mineral_model::Artist::builder()
        .id(ArtistId::new(SourceKind::NETEASE, "r1"))
        .name("artist".to_owned())
        .songs(vec![song("1")])
        .build();
    for (index, ctx, expected) in [
        (0, copy_song(), "netease:1"),
        (
            1,
            CopyTemplateCtx::Playlist(Box::new(playlist)),
            "netease:p1",
        ),
        (2, CopyTemplateCtx::Album(Box::new(album)), "netease:a1"),
        (3, CopyTemplateCtx::Artist(Box::new(artist)), "netease:r1"),
    ] {
        let (text, commands) = loaded.runtime.render_copy_template(index, ctx)?;
        assert_eq!(text, expected);
        assert!(commands.is_empty());
    }
    Ok(())
}

/// 测试用音乐 VM,用于证明两个 require 模块不会进入同一个宿主。
fn vm_with_host() -> color_eyre::Result<(mineral_script::mlua::Lua, mineral_script::ScriptHost)> {
    let (cmd_tx, _commands) = tokio::sync::mpsc::unbounded_channel();
    let (push_tx, _failures) = tokio::sync::mpsc::unbounded_channel();
    let host = mineral_script::ScriptHost::new(cmd_tx, push_tx);
    let lua = mineral_script::mlua::Lua::new();
    mineral_script::install_daemon_api(&lua, &host)?;
    Ok((lua, host))
}

/// 短回调的测试预算;不作为应用配置默认值。
fn lax_watchdog() -> WatchdogConfig {
    WatchdogConfig::builder()
        .instruction_interval(10_000)
        .soft_wall(std::time::Duration::from_millis(200))
        .hard_wall(std::time::Duration::from_secs(1))
        .build()
}
