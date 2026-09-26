//! 真实按键入口的播放命令路由、输入消费和后端确认行为。

use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mineral_protocol::{BusValue, PlayMode, PlayerSync};

use super::App;
use crate::test_support::{TestClient, app_with_queue};

/// 从真实事件入口送入一个默认无修饰按键。
fn press(app: &mut App, key: char) {
    app.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char(key),
        KeyModifiers::NONE,
    )));
}

/// Seek 按键发送目标位置，后端确认前不改变播放位置；连按仍各发送一次。
#[test]
fn seek_keys_send_targets_without_predicting_position() -> color_eyre::Result<()> {
    for (key, modifiers, target) in [
        (KeyCode::Left, KeyModifiers::NONE, 55_000),
        (KeyCode::Right, KeyModifiers::NONE, 65_000),
        (KeyCode::Left, KeyModifiers::SHIFT, 30_000),
        (KeyCode::Right, KeyModifiers::SHIFT, 90_000),
    ] {
        let (mut app, seeks) = crate::test_support::app_in_fullscreen_seek_probe()?;
        app.state.playback.position_ms = 60_000;
        let event = Event::Key(KeyEvent::new(key, modifiers));
        app.handle_event(&event);
        assert_eq!(
            seeks.lock().ok().as_deref().map(Vec::as_slice),
            Some([target].as_slice())
        );
        assert_eq!(app.state.playback.position_ms, 60_000);
        for _ in 0..6 {
            app.state.tick_frame();
        }
        app.handle_event(&event);
        assert_eq!(
            seeks.lock().ok().as_deref().map(Vec::as_slice),
            Some([target, target].as_slice())
        );
        assert_eq!(app.state.playback.position_ms, 60_000);
    }
    Ok(())
}

/// 首次按键发送一次命令；等待确认时播放状态与模式不提前变化。
#[test]
fn first_control_key_executes_once_and_waits_for_confirmation() -> color_eyre::Result<()> {
    for (key, expected) in [
        ('p', "prev_or_restart"),
        (' ', "resume"),
        ('n', "next_song"),
        ('m', "cycle_play_mode"),
    ] {
        let mut app = app_with_queue(3, 0)?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        app.client = Arc::new(TestClient {
            playback_controls: Arc::clone(&calls),
            ..TestClient::default()
        });
        press(&mut app, key);
        assert_eq!(
            calls.lock().ok().as_deref().map(Vec::as_slice),
            Some([expected].as_slice())
        );
        for _ in 0..30 {
            app.state.tick_frame();
        }
        assert!(!app.state.playback.playing);
        assert_eq!(app.state.playback.mode, PlayMode::Sequential);
    }
    Ok(())
}

/// 重映射生效，搜索和确认浮层消费的字符不发送播放命令。
#[test]
fn remapped_actions_and_consumed_text_follow_existing_routing() -> color_eyre::Result<()> {
    let mut app = app_with_queue(3, 0)?;
    let calls = Arc::new(Mutex::new(Vec::new()));
    app.client = Arc::new(TestClient {
        playback_controls: Arc::clone(&calls),
        ..TestClient::default()
    });
    let config = mineral_config::merge_tree(
        mineral_config::default_tree()?,
        serde_json::json!({"tui": {"keys": {"cycle_mode": "w"}}}),
    );
    app.apply_pushed_config(BusValue::from_json(config));
    for key in ['m', '+', 'j'] {
        press(&mut app, key);
    }
    assert!(calls.lock().is_ok_and(|log| log.is_empty()));
    press(&mut app, 's');
    press(&mut app, 'w');
    assert!(calls.lock().is_ok_and(|log| log.is_empty()));
    app.handle_event(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    for _ in 0..30 {
        app.state.tick_frame();
    }
    press(&mut app, 'q');
    press(&mut app, 'w');
    assert!(calls.lock().is_ok_and(|log| log.is_empty()));
    app.handle_event(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    for _ in 0..30 {
        app.overlays.tick();
    }
    press(&mut app, 'w');
    assert_eq!(
        calls.lock().ok().as_deref().map(Vec::as_slice),
        Some(["cycle_play_mode"].as_slice())
    );
    Ok(())
}

/// 命令提交后保持确认值，后端回包经镜像入口更新播放状态。
#[test]
fn volume_and_mode_update_after_backend_confirmation() -> color_eyre::Result<()> {
    let (mut app, volumes) = crate::test_support::app_with_queue_volume_probed(1, 0)?;
    app.state.playback.volume_pct = 50;
    press(&mut app, '+');
    press(&mut app, 'm');
    assert_eq!(
        volumes
            .lock()
            .ok()
            .and_then(|values| values.last().copied()),
        Some(55)
    );
    assert_eq!(app.state.playback.volume_pct, 50);
    assert_eq!(app.state.playback.mode, PlayMode::Sequential);
    app.state
        .playback
        .apply_audio_snapshot(mineral_audio::AudioSnapshot {
            volume_pct: 55,
            playing: true,
            ..Default::default()
        });
    app.apply_player_sync(PlayerSync {
        play_mode: PlayMode::RepeatOne,
        ..Default::default()
    });
    assert_eq!(app.state.playback.volume_pct, 55);
    assert_eq!(app.state.playback.mode, PlayMode::RepeatOne);
    assert!(app.state.playback.playing);
    Ok(())
}
