//! 配置重载保留本地反馈的相位与期限。

use super::ControlButton;
use crate::runtime::action::{Action, VolumeDelta};
use mineral_protocol::PlayMode;
use std::time::Instant;

/// 在途反馈通过 App::apply_config 换速，当前画面和已建立期限都不重置。
#[test]
fn app_reload_preserves_feedback_phase_and_existing_deadlines() -> color_eyre::Result<()> {
    use mineral_protocol::BusValue;
    let mut app = crate::test_support::app_with_queue(1, 0)?;
    let now = Instant::now();
    let anim = app.state.cfg.tui().animation();
    app.state.transport.on_action(
        Action::NudgeVolume(VolumeDelta(5)),
        PlayMode::Sequential,
        anim,
        now,
    );
    app.state
        .transport
        .on_action(Action::CyclePlayMode, PlayMode::Sequential, anim, now);
    app.state.transport.on_seek(anim);
    for _ in 0..6 {
        app.state.transport.tick(PlayMode::RepeatAll, anim, now);
    }
    let before = app.state.transport.clone();
    let tree = mineral_config::merge_tree(
        mineral_config::default_tree()?,
        serde_json::json!({
            "tui": { "animation": { "frame_tick_ms": 32, "controls_press_ms": 4400, "transport": {
                "volume_fade_out_ms": 2200, "volume_fade_in_ms": 3000,
                "mode_reveal_ms": 4400, "mode_resize_ms": 4000, "controls_fade_ms": 4400,
                "volume_hold_ms": 6000, "mode_hold_ms": 6000, "controls_hold_ms": 9000
            } } }
        }),
    );
    app.apply_pushed_config(BusValue::from_json(tree));
    let after = &app.state.transport;
    assert_eq!(after.heading(), before.heading());
    assert_eq!(after.mode_caption(), before.mode_caption());
    assert_eq!(after.controls_opacity(), before.controls_opacity());
    assert_eq!(after.volume_until, before.volume_until);
    assert_eq!(after.mode_until, before.mode_until);
    assert_eq!(after.controls_until, before.controls_until);
    assert_eq!(
        after.elapsed_press_strength(),
        before.elapsed_press_strength()
    );
    for button in [
        ControlButton::Previous,
        ControlButton::PlayPause,
        ControlButton::Next,
        ControlButton::Mode,
    ] {
        assert_eq!(after.button(button), before.button(button));
    }
    let old_cfg = mineral_config::Config::defaults()?;
    let before_opacity = before.controls_opacity();
    let before_reveal = before.mode_caption().1;
    let before_width = before.mode.width.current();
    let before_press = before.button(ControlButton::Mode).press_strength;
    let before_elapsed_press = before.elapsed_press_strength();
    let mut old_speed = before;
    old_speed.tick(PlayMode::RepeatAll, old_cfg.tui().animation(), now);
    app.state
        .transport
        .tick(PlayMode::RepeatAll, app.state.cfg.tui().animation(), now);
    assert!(app.state.transport.controls_opacity() > before_opacity);
    assert!(app.state.transport.controls_opacity() < old_speed.controls_opacity());
    assert!(app.state.transport.mode_caption().1 > before_reveal);
    assert!(app.state.transport.mode_caption().1 < old_speed.mode_caption().1);
    assert!(app.state.transport.mode.width.current() > before_width);
    assert!(app.state.transport.mode.width.current() < old_speed.mode.width.current());
    let after_press = app
        .state
        .transport
        .button(ControlButton::Mode)
        .press_strength;
    assert!(after_press < before_press);
    assert!(after_press > old_speed.button(ControlButton::Mode).press_strength);
    let after_elapsed_press = app.state.transport.elapsed_press_strength();
    assert!(after_elapsed_press < before_elapsed_press);
    assert!(after_elapsed_press > old_speed.elapsed_press_strength());
    Ok(())
}
