//! App input and local file reload tests; daemon messages carry business capabilities only.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mineral_protocol::{PlayCountAvailability, ServiceInfo};
use mineral_script::CopyTemplateCtx;

use crate::app::App;
use crate::runtime::tui_script::{ERROR_CARD_ID, TuiStartup};
use crate::test_support::{
    TestClient, app_with_library, app_with_queue, app_with_queue_volume_probed,
};

/// Advance beyond the local file polling interval without sleeping.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Write a deterministically distinct mtime version, independent of filesystem resolution.
fn write_entry(path: &Path, source: &str, version: u64) -> color_eyre::Result<()> {
    std::fs::write(path, source)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(version)),
        )?;
    Ok(())
}

/// Send an actual unmodified key through App routing.
fn press(app: &mut App, key: char) {
    app.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char(key),
        KeyModifiers::NONE,
    )));
}

/// Copy context already held by the TUI, requiring no daemon query.
fn song_context() -> CopyTemplateCtx {
    CopyTemplateCtx::Song(Box::new(mineral_test::song("s1")))
}

/// Startup evaluates the file once and carries its setup state into local copy callbacks.
#[test]
fn startup_keeps_one_evaluation_and_applies_setup_before_resource_construction()
-> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tui.lua");
    let marker = dir.path().join("evaluations");
    write_entry(
        &path,
        &format!(
            r#"
        local file = assert(io.open({:?}, "a"))
        file:write("x")
        file:close()
        local calls = 0
        return {{
            setup = function(api)
                calls = calls + 1
                api.config.override({{ behavior = {{ volume_step = 21 }} }})
            end,
            copy = {{ templates = {{{{
                label = "Calls", template = function(song)
                    calls = calls + 1
                    return song.id .. ":" .. calls
                end,
            }}}} }},
        }}
    "#,
            marker.to_string_lossy()
        ),
        1,
    )?;
    let startup = TuiStartup::load(path)?;
    assert_eq!(*startup.config.behavior().volume_step(), 21);
    let mut app = app_with_queue(1, 0)?;
    app.apply_tui_startup(startup);
    app.poll_tui_script(Instant::now() + POLL_INTERVAL);
    assert_eq!(std::fs::metadata(marker)?.len(), 1);
    assert_eq!(
        app.render_local_copy_template(0, song_context())?,
        format!("{}:2", mineral_test::song("s1").id.qualified())
    );
    Ok(())
}

/// Effective config updates change callback budgets without replacing the callback VM.
#[test]
fn local_config_reconfigures_copy_watchdog() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tui.lua");
    write_entry(
        &path,
        r#"return { copy = { templates = {{
        label = "Identity", template = function(song) return song.id end,
    }} } }"#,
        1,
    )?;
    let mut app = app_with_queue(1, 0)?;
    app.start_tui_script(path);
    let original = Arc::clone(&app.state.cfg);
    assert!(app.render_local_copy_template(0, song_context()).is_ok());
    let tree = mineral_config::merge_tree(
        crate::config::default_tui_tree()?,
        serde_json::json!({"script": {
            "watchdog_instruction_interval": 1,
            "watchdog_soft_wall_ms": 0,
            "watchdog_hard_wall_ms": 0
        }}),
    );
    app.apply_config(Arc::new(crate::config::tui_from_tree(&tree)?));
    assert!(app.render_local_copy_template(0, song_context()).is_err());
    app.apply_config(original);
    assert_eq!(
        app.render_local_copy_template(0, song_context())?,
        mineral_test::song("s1").id.qualified()
    );
    Ok(())
}

/// Distinct TUI files apply immediately and keep config, callbacks and UI effects private.
#[test]
fn two_apps_keep_tui_settings_and_callbacks_private() -> color_eyre::Result<()> {
    let first_dir = tempfile::tempdir()?;
    let second_dir = tempfile::tempdir()?;
    let first_path = first_dir.path().join("tui.lua");
    let second_path = second_dir.path().join("tui.lua");
    write_entry(
        &first_path,
        r#"
        local calls = 0
        return {
            behavior = { volume_step = 17 },
            setup = function(api)
                api.ui.window_title("first tui")
                api.ui.toast("ready", { id = "ready" })
                api.ui.card { body = { "local" }, id = "ready" }
            end,
            copy = { templates = {{ context = "song", label = "Counter", template = function(song)
                calls = calls + 1
                return song.id .. ":" .. calls
            end }} }
        }
    "#,
        1,
    )?;
    write_entry(
        &second_path,
        r#"
        return {
            behavior = { volume_step = 33 },
            setup = function(api) api.ui.window_title("second tui") end,
            copy = { templates = {{ context = "song", label = "Identity", template = function(song)
                return song.id
            end }} }
        }
    "#,
        1,
    )?;
    let mut first = app_with_queue(1, 0)?;
    let mut second = app_with_queue(1, 0)?;
    let shared_backend = Arc::new(TestClient::default());
    first.client = shared_backend.clone();
    second.client = shared_backend;
    first.start_tui_script(first_path);
    second.start_tui_script(second_path.clone());
    assert_eq!(*first.state.cfg.behavior().volume_step(), 17);
    assert_eq!(*second.state.cfg.behavior().volume_step(), 33);
    assert!(first.tui_script.title.is_some());
    assert_ne!(first.tui_script.title, second.tui_script.title);
    assert!(first.notifications.has_live_card("ready"));
    assert!(!second.notifications.has_live_card("ready"));
    let id = mineral_test::song("s1").id.qualified();
    assert_eq!(
        first.render_local_copy_template(0, song_context())?,
        format!("{id}:1")
    );
    assert_eq!(second.render_local_copy_template(0, song_context())?, id);
    assert_eq!(
        first.render_local_copy_template(0, song_context())?,
        format!("{id}:2")
    );

    let first_cfg = Arc::clone(&first.state.cfg);
    let first_title = first.tui_script.title.clone();
    std::fs::remove_file(second_path)?;
    second.poll_tui_script(Instant::now() + POLL_INTERVAL);
    assert!(Arc::ptr_eq(&first_cfg, &first.state.cfg));
    assert_eq!(first.tui_script.title, first_title);
    assert!(second.tui_script.title.is_none());
    assert!(
        second
            .render_local_copy_template(0, song_context())
            .is_err()
    );
    Ok(())
}

/// Service info updates change daemon operations but cannot replace local keys or preferences.
#[test]
fn daemon_service_updates_do_not_overwrite_local_preferences() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tui.lua");
    write_entry(
        &path,
        r#"
        return {
            behavior = { volume_step = 18 },
            keys = { volume_up = "w" },
            heartbeat_secs = 19,
            setup = function(api) api.ui.window_title("local") end
        }
    "#,
        1,
    )?;
    let (mut app, volumes) = app_with_queue_volume_probed(1, 0)?;
    app.state.models.playback.volume_pct = 10;
    app.start_tui_script(path);
    let local_cfg = Arc::clone(&app.state.cfg);
    let local_title = app.tui_script.title.clone();
    let client = Arc::new(TestClient {
        volumes: Arc::clone(&volumes),
        ..TestClient::default()
    });
    app.client = client.clone();
    let info = ServiceInfo {
        queue_transforms: vec!["Reverse queue".to_owned()],
        play_counts: PlayCountAvailability {
            enabled: true,
            excluded_sources: vec!["local".to_owned()],
        },
    };
    client
        .events
        .lock()
        .map_err(|error| color_eyre::eyre::eyre!("{error}"))?
        .push(mineral_protocol::Event::ServiceInfoChanged { info: info.clone() });
    app.drain_push_events();
    assert_eq!(app.state.models.service_info, info);
    assert!(Arc::ptr_eq(&local_cfg, &app.state.cfg));
    assert_eq!(app.tui_script.title, local_title);
    press(&mut app, '+');
    press(&mut app, 'w');
    assert_eq!(
        *volumes
            .lock()
            .map_err(|error| color_eyre::eyre::eyre!("{error}"))?,
        vec![28]
    );
    Ok(())
}

/// File/setup/override failures preserve config, title and callback VM; deletion restores defaults.
#[test]
fn tui_file_reload_is_transactional_and_deletion_clears_settings() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tui.lua");
    write_entry(
        &path,
        r#"
        return {
            behavior = { volume_step = 18 }, keys = { volume_up = "w" },
            setup = function(api)
                api.ui.window_title("first")
                api.ui.toast("ready", { id = "ready" })
                api.ui.card { body = { "ready" }, id = "ready" }
            end,
            copy = { templates = {{ context = "song", label = "Local", template = function()
                return "first callback"
            end }} }
        }
    "#,
        1,
    )?;
    let (mut app, volumes) = app_with_queue_volume_probed(1, 0)?;
    app.state.models.playback.volume_pct = 10;
    app.start_tui_script(path.clone());
    press(&mut app, 'w');
    let title = app.tui_script.title.clone();
    let local_cfg = Arc::clone(&app.state.cfg);
    let mut now = Instant::now();

    let invalid_files = [
        "error('top-level failure')",
        "return { behavior = { volume_step = 'invalid' } }",
        "return { audio = { volume = 7 } }",
        "return { tui = { behavior = { volume_step = 40 } } }",
        "return { daemon = {} }",
        "return { setup = false }",
    ];
    let invalid_setups = [
        "error('setup failure')",
        "api.config.override('behavior.volume_step', 'invalid')",
        "api.config.override('audio.volume', 7)",
        "api.config.override('tui.behavior.volume_step', 40)",
        "api.config.override({ daemon = { heartbeat_secs = 7 } })",
        "api.config.override('copy', {templates = {{label = 'replacement'}}})",
        "api.config.override('copy.templates', {{label = 'replacement'}})",
        "api.config.override({copy = {templates = {{label = 'replacement'}}}})",
    ];
    let sources =
        invalid_files
            .into_iter()
            .map(str::to_owned)
            .chain(invalid_setups.into_iter().map(|failure| {
                format!(
                    r#"
                return {{ setup = function(api)
                    api.config.override("behavior.volume_step", 40)
                    api.ui.window_title("must not commit")
                    api.ui.toast("must not commit")
                    api.ui.card {{ body = {{ "must not commit" }}, id = "failed-effect" }}
                    {failure}
                end }}
            "#
                )
            }));
    for (index, source) in sources.enumerate() {
        write_entry(&path, &source, u64::try_from(index)? + 2)?;
        now += POLL_INTERVAL;
        app.poll_tui_script(now);
        assert!(Arc::ptr_eq(&local_cfg, &app.state.cfg));
        assert_eq!(app.tui_script.title, title);
        assert!(app.notifications.has_live_card("ready"));
        assert!(app.notifications.has_live_card(ERROR_CARD_ID));
        assert!(!app.notifications.has_live_card("failed-effect"));
        assert_eq!(app.notifications.entry_count(), 1);
        assert_eq!(
            app.render_local_copy_template(0, song_context())?,
            "first callback"
        );
    }
    write_entry(
        &path,
        r#"
        return {
            behavior = { volume_step = 24 }, keys = { volume_up = "e" },
            setup = function(api)
                api.ui.window_title("second")
                api.ui.card { body = { "reloaded" }, id = "ready" }
            end,
            copy = { templates = {{ context = "song", label = "Local", template = function()
                return "second callback"
            end }} }
        }
    "#,
        u64::try_from(invalid_files.len() + invalid_setups.len())? + 2,
    )?;
    now += POLL_INTERVAL;
    app.poll_tui_script(now);
    assert_ne!(app.tui_script.title, title);
    assert!(!app.notifications.has_live_card(ERROR_CARD_ID));
    assert!(app.notifications.has_live_card("ready"));
    press(&mut app, 'w');
    press(&mut app, 'e');
    assert_eq!(
        app.render_local_copy_template(0, song_context())?,
        "second callback"
    );

    std::fs::remove_file(path)?;
    now += POLL_INTERVAL;
    app.poll_tui_script(now);
    let defaults = crate::config::TuiConfig::defaults()?;
    assert_eq!(
        app.state.cfg.behavior().volume_step(),
        defaults.behavior().volume_step(),
    );
    assert!(app.tui_script.title.is_none());
    assert!(app.render_local_copy_template(0, song_context()).is_err());
    press(&mut app, 'e');
    press(&mut app, '+');
    assert_eq!(
        *volumes
            .lock()
            .map_err(|error| color_eyre::eyre::eyre!("{error}"))?,
        vec![28, 34, 10 + *defaults.behavior().volume_step()]
    );
    assert!(!app.notifications.has_live_card(ERROR_CARD_ID));
    Ok(())
}

/// Local copy callbacks keep closure state, seeded URLs and transactional TUI effects.
#[test]
fn local_copy_callbacks_validate_effects_before_committing() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tui.lua");
    write_entry(
        &path,
        r#"
        local api
        local calls = 0
        return {
            behavior = { volume_step = 18 },
            setup = function(host) api = host end,
            copy = { templates = {
                { context = "song", label = "Count", template = function(song)
                    calls = calls + 1
                    api.config.override("behavior.volume_step", calls == 1 and 27 or nil)
                    api.ui.window_title("copy title")
                    api.ui.card { body = { "copied" }, id = "copy" }
                    return song.url .. ":" .. calls
                end },
                { context = "song", label = "Invalid", template = function()
                    api.ui.window_title("must not commit")
                    api.ui.card { body = { "failed" }, id = "copy-failed" }
                    api.config.override("behavior.volume_step", "invalid")
                    return "must not copy"
                end },
                { context = "song", key = "w", label = "Error", template = function()
                    api.ui.window_title("must not commit")
                    api.ui.card { body = { "failed" }, id = "copy-failed" }
                    error("callback failure")
                end }
            } }
        }
    "#,
        1,
    )?;
    let mut app = app_with_library(1, 0)?;
    let song = mineral_test::song("s1");
    app.state.models.caps.insert(
        song.source(),
        mineral_channel_core::ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .artist_sections(mineral_channel_core::ArtistSections::new(Vec::new()))
            .song_web_url(Some("https://music.example/song/{id}".to_owned()))
            .build(),
    );
    app.start_tui_script(path);
    let url = format!("https://music.example/song/{}", song.id.value());
    assert_eq!(
        app.render_local_copy_template(0, song_context())?,
        format!("{url}:1")
    );
    assert_eq!(*app.state.cfg.behavior().volume_step(), 27);
    assert!(app.notifications.has_live_card("copy"));
    assert!(app.tui_script.title.is_some());
    assert_eq!(
        app.render_local_copy_template(0, song_context())?,
        format!("{url}:2")
    );
    assert_eq!(*app.state.cfg.behavior().volume_step(), 18);
    let cfg = Arc::clone(&app.state.cfg);
    let title = app.tui_script.title.clone();
    assert!(app.render_local_copy_template(1, song_context()).is_err());
    assert!(Arc::ptr_eq(&cfg, &app.state.cfg));
    assert_eq!(app.tui_script.title, title);
    assert!(!app.notifications.has_live_card("copy-failed"));
    press(&mut app, 'y');
    press(&mut app, 'w');
    assert_eq!(app.tui_script.title, title);
    assert!(!app.notifications.has_live_card("copy-failed"));
    assert!(app.clipboard.is_none());
    assert!(app.completions.drain().is_empty());
    Ok(())
}

/// Availability changes clear stale counts, re-query available sources and preserve local config.
#[test]
fn service_info_changes_refresh_play_counts() -> color_eyre::Result<()> {
    let mut app = app_with_library(1, 0)?;
    let client = Arc::new(TestClient::default());
    app.client = client.clone();
    let cfg = Arc::clone(&app.state.cfg);
    let id = app
        .state
        .models
        .library
        .tracks
        .values()
        .flat_map(|tracks| tracks.iter())
        .next()
        .ok_or_else(|| color_eyre::eyre::eyre!("missing fixture song"))?
        .data
        .song
        .id
        .clone();
    let enabled = ServiceInfo {
        queue_transforms: Vec::new(),
        play_counts: PlayCountAvailability {
            enabled: true,
            excluded_sources: Vec::new(),
        },
    };
    app.apply_service_info(enabled.clone());
    app.state
        .apply(&mineral_task::TaskEvent::LocalPlayCountFetched {
            song_id: id.clone(),
            count: Some(3),
        });
    assert_eq!(
        app.state.models.library.local_play_counts.get(&id),
        Some(&3)
    );
    let mut excluded = enabled.clone();
    excluded
        .play_counts
        .excluded_sources
        .push(id.namespace().name().to_owned());
    app.apply_service_info(excluded);
    assert!(
        app.state
            .models
            .library
            .local_play_counts
            .has_no_cached_values()
    );
    assert!(
        app.state
            .models
            .library
            .tracks
            .values()
            .flat_map(|tracks| tracks.iter())
            .all(|entry| entry.plays.is_none())
    );
    app.apply_service_info(enabled);
    let requests = client
        .song_stats_requests
        .lock()
        .map_err(|error| color_eyre::eyre::eyre!("{error}"))?;
    assert_eq!(*requests, vec![id.clone(), id]);
    assert!(Arc::ptr_eq(&cfg, &app.state.cfg));
    Ok(())
}
