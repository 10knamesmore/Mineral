//! Action 路由与播放栏反馈集成：首次按键执行命令，输入消费与确认态保持原契约。

use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mineral_protocol::{BusValue, PlayMode, PlayerSync};
use ratatui::{Terminal, backend::TestBackend};

use super::App;
use crate::test_support::{TestClient, app_with_queue};

/// 从真实事件入口送入一个默认无修饰按键。
fn press(app: &mut App, key: char) {
    app.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char(key),
        KeyModifiers::NONE,
    )));
}

/// 从实际 transport renderer 获取五行文本，不经过其他组件的准备或绘制。
fn transport_text(app: &App) -> color_eyre::Result<String> {
    use crate::components::layout::shared::{
        marquee::MarqueeCtx, transport, waveform::WaveformCtx,
    };
    let mut terminal = Terminal::new(TestBackend::new(60, 5))?;
    terminal.draw(|frame| {
        transport::draw(
            frame,
            frame.area(),
            &app.state.playback,
            &app.state.transport,
            &MarqueeCtx::new(&app.state, &app.theme, app.theme.base),
            &WaveformCtx::new(&app.state, &app.theme),
            &app.theme,
        )
    })?;
    Ok(terminal.backend().to_string())
}

/// 按键唤出反馈的同时只发送一次原命令；等待确认时播放图标与模式不提前变化。
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
        assert_eq!(app.state.transport.controls_opacity(), 0);
        press(&mut app, key);
        assert_eq!(
            calls.lock().ok().as_deref().map(Vec::as_slice),
            Some([expected].as_slice())
        );
        for _ in 0..30 {
            app.state.tick_frame();
        }
        assert_eq!(app.state.transport.controls_opacity(), 1000);
        assert!(!app.state.playback.playing, "resume 尚未确认");
        assert_eq!(app.state.playback.mode, PlayMode::Sequential);
        let waiting = transport_text(&app)?;
        assert!(waiting.contains("[▶]"));
        if key == 'm' {
            assert!(waiting.contains("seq"));
        }
    }
    Ok(())
}

/// 自定义键位跟随 Action；音量、无关键、搜索输入与确认浮层消费的键不唤起控件。
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
        serde_json::json!({
            "tui": { "keys": { "cycle_mode": "w" } }
        }),
    );
    app.apply_pushed_config(BusValue::from_json(config));
    for key in ['m', '+', 'j'] {
        press(&mut app, key);
    }
    for _ in 0..30 {
        app.state.tick_frame();
    }
    assert_eq!(app.state.transport.controls_opacity(), 0);
    assert!(calls.lock().is_ok_and(|log| log.is_empty()));
    press(&mut app, 's');
    press(&mut app, 'w');
    for _ in 0..30 {
        app.state.tick_frame();
    }
    assert_eq!(
        app.state.transport.controls_opacity(),
        0,
        "搜索 prompt 消费字符"
    );
    assert!(calls.lock().is_ok_and(|log| log.is_empty()));
    app.handle_event(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    for _ in 0..30 {
        app.state.tick_frame();
    }
    press(&mut app, 'q');
    press(&mut app, 'w');
    for _ in 0..30 {
        app.state.tick_frame();
    }
    assert_eq!(
        app.state.transport.controls_opacity(),
        0,
        "确认浮层消费按键"
    );
    app.handle_event(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    for _ in 0..30 {
        app.overlays.tick();
    }
    press(&mut app, 'w');
    for _ in 0..30 {
        app.state.tick_frame();
    }
    assert_eq!(app.state.transport.controls_opacity(), 1000);
    assert_eq!(
        calls.lock().ok().as_deref().map(Vec::as_slice),
        Some(["cycle_play_mode"].as_slice())
    );
    Ok(())
}

/// 提示出现仍显示确认值，后端值到达后经现有镜像入口更新。
#[test]
fn delayed_volume_and_mode_confirmation_updates_visible_feedback() -> color_eyre::Result<()> {
    let (mut app, volumes) = crate::test_support::app_with_queue_volume_probed(1, 0)?;
    app.state.playback.volume_pct = 50;
    press(&mut app, '+');
    press(&mut app, 'm');
    for _ in 0..30 {
        app.state.tick_frame();
    }
    assert_eq!(
        volumes
            .lock()
            .ok()
            .and_then(|values| values.last().copied()),
        Some(55)
    );
    let waiting = transport_text(&app)?;
    assert!(waiting.contains("50%"));
    assert!(!waiting.contains("55%"));
    assert!(waiting.contains("seq"));
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
    for _ in 0..30 {
        app.state.tick_frame();
    }
    let confirmed = transport_text(&app)?;
    assert!(confirmed.contains("55%"));
    assert!(confirmed.contains("rep one"));
    assert!(confirmed.contains("[⏸]"));
    Ok(())
}
