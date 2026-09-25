//! 本地反馈的计时、确认态与中断接续回归。

use super::{Heading, TransportFeedback};
use crate::runtime::action::{Action, VolumeDelta};
use mineral_config::AnimationConfig;
use mineral_protocol::PlayMode;
use std::time::{Duration, Instant};

/// 在固定时刻推进足够多帧使短动画落定，不消耗提示期限。
fn settle(feedback: &mut TransportFeedback, mode: PlayMode, anim: &AnimationConfig, now: Instant) {
    for _ in 0..80 {
        feedback.tick(mode, anim, now);
    }
}

/// 音量不唤起底部按钮；模式标签先收，按钮停留更久；同步不延长期限。
#[test]
fn independent_deadlines_restore_the_idle_border() -> color_eyre::Result<()> {
    let cfg = mineral_config::Config::defaults()?;
    let anim = cfg.tui().animation();
    let now = Instant::now();
    let mut feedback = TransportFeedback::new(PlayMode::Sequential, anim);
    assert_eq!(feedback.controls_opacity(), 0);
    feedback.on_action(
        Action::NudgeVolume(VolumeDelta(5)),
        PlayMode::Sequential,
        anim,
        now,
    );
    settle(&mut feedback, PlayMode::Sequential, anim, now);
    assert_eq!(feedback.heading(), (Heading::Volume, 1000));
    assert_eq!(feedback.controls_opacity(), 0);

    let volume_hold = Duration::from_millis(u64::from(*anim.transport().volume_hold_ms()));
    let mode_hold = Duration::from_millis(u64::from(*anim.transport().mode_hold_ms()));
    let after_volume = now + volume_hold;
    let later = after_volume - mode_hold / 2;
    feedback.on_action(Action::CyclePlayMode, PlayMode::Sequential, anim, later);
    settle(&mut feedback, PlayMode::RepeatAll, anim, later);
    assert!(feedback.mode_caption().0.expanded);
    assert_eq!(feedback.controls_opacity(), 1000);
    settle(&mut feedback, PlayMode::RepeatAll, anim, after_volume);
    assert_eq!(feedback.heading(), (Heading::Transport, 1000));
    assert!(
        feedback.mode_caption().0.expanded,
        "volume 到期不消费 mode 期限"
    );
    let after_mode = later + mode_hold;
    settle(&mut feedback, PlayMode::RepeatAll, anim, after_mode);
    assert!(!feedback.mode_caption().0.expanded);
    assert_eq!(feedback.mode_caption().2, 2, "循环模式合拢为双格图标");
    assert_eq!(feedback.controls_opacity(), 1000);
    let after_controls =
        later + Duration::from_millis(u64::from(*anim.transport().controls_hold_ms()));
    settle(&mut feedback, PlayMode::RepeatAll, anim, after_controls);
    assert_eq!(
        feedback.controls_opacity(),
        0,
        "重复确认态同步不能让按钮常驻"
    );
    Ok(())
}

/// 控制退场中重入保留当前亮度，只延长控制期限，不重新展开已收起的模式文字。
#[test]
fn repeated_controls_extend_hold_and_reverse_from_current_opacity() -> color_eyre::Result<()> {
    let cfg = mineral_config::Config::defaults()?;
    let anim = cfg.tui().animation();
    let now = Instant::now();
    let mut feedback = TransportFeedback::new(PlayMode::Sequential, anim);
    feedback.on_action(Action::CyclePlayMode, PlayMode::Sequential, anim, now);
    settle(&mut feedback, PlayMode::Sequential, anim, now);
    let deadline = now + Duration::from_millis(u64::from(*anim.transport().controls_hold_ms()));
    for _ in 0..5 {
        feedback.tick(PlayMode::Sequential, anim, deadline);
    }
    let mid = feedback.controls_opacity();
    assert!(mid > 0 && mid < 1000);
    feedback.on_action(Action::NextSong, PlayMode::Sequential, anim, deadline);
    assert_eq!(feedback.controls_opacity(), mid, "重入那一刻不跳回端点");
    feedback.tick(PlayMode::Sequential, anim, deadline);
    assert!(feedback.controls_opacity() > mid);
    settle(&mut feedback, PlayMode::Sequential, anim, deadline);
    assert!(!feedback.mode_caption().0.expanded, "next 不续期模式短标签");
    assert_eq!(feedback.controls_opacity(), 1000);
    let renewed = deadline + Duration::from_millis(u64::from(*anim.transport().controls_hold_ms()));
    settle(&mut feedback, PlayMode::Sequential, anim, renewed);
    assert_eq!(feedback.controls_opacity(), 0);
    Ok(())
}

/// 标题两个方向都有中间亮度，淡出尚未换字时反向沿当前亮度恢复。
#[test]
fn title_crossfade_can_reverse_without_restarting() -> color_eyre::Result<()> {
    let cfg = mineral_config::Config::defaults()?;
    let anim = cfg.tui().animation();
    let now = Instant::now();
    let mut feedback = TransportFeedback::new(PlayMode::Sequential, anim);
    feedback.on_action(
        Action::NudgeVolume(VolumeDelta(5)),
        PlayMode::Sequential,
        anim,
        now,
    );
    feedback.tick(PlayMode::Sequential, anim, now);
    let fading = feedback.heading();
    assert_eq!(fading.0, Heading::Transport);
    assert!(fading.1 > 0 && fading.1 < 1000);
    settle(&mut feedback, PlayMode::Sequential, anim, now);
    let deadline = now + Duration::from_millis(u64::from(*anim.transport().volume_hold_ms()));
    feedback.tick(PlayMode::Sequential, anim, deadline);
    let leaving = feedback.heading();
    assert_eq!(leaving.0, Heading::Volume);
    assert!(leaving.1 > 0 && leaving.1 < 1000);
    feedback.on_action(
        Action::NudgeVolume(VolumeDelta(-5)),
        PlayMode::Sequential,
        anim,
        deadline,
    );
    assert_eq!(feedback.heading(), leaving);
    feedback.tick(PlayMode::Sequential, anim, deadline);
    assert!(feedback.heading().1 > leaving.1);
    Ok(())
}

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
    for _ in 0..3 {
        app.state.transport.tick(PlayMode::RepeatAll, anim, now);
    }
    let before = app.state.transport.clone();
    let tree = mineral_config::merge_tree(
        mineral_config::default_tree()?,
        serde_json::json!({
            "tui": { "animation": { "frame_tick_ms": 32, "transport": {
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
    let old_cfg = mineral_config::Config::defaults()?;
    let before_opacity = before.controls_opacity();
    let before_reveal = before.mode_caption().1;
    let before_width = before.mode.width.current();
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
    Ok(())
}

/// 动作不预测模式；确认值立即显示，中断时只揭示最新标签，括号保持当前宽度。
#[test]
fn mode_waits_for_confirmation_and_coalesces_interrupted_targets() -> color_eyre::Result<()> {
    let cfg = mineral_config::Config::defaults()?;
    let anim = cfg.tui().animation();
    let now = Instant::now();
    let mut feedback = TransportFeedback::new(PlayMode::Sequential, anim);
    feedback.on_action(Action::CyclePlayMode, PlayMode::Sequential, anim, now);
    settle(&mut feedback, PlayMode::Sequential, anim, now);
    assert_eq!(
        feedback.mode_caption().0.mode,
        PlayMode::Sequential,
        "动作不能预测模式"
    );
    feedback.sync_mode(PlayMode::RepeatAll, anim);
    assert_eq!(feedback.mode_caption().0.mode, PlayMode::RepeatAll);
    assert!(feedback.mode_caption().2 >= 2, "双列图标必须完整");
    for _ in 0..3 {
        feedback.tick(PlayMode::RepeatAll, anim, now);
    }
    let before = feedback.mode_caption();
    assert!(before.1 > 0 && before.1 < 1000, "文字正在逐列显现");
    assert!(before.0.label_opacity(0, before.1) > before.0.label_opacity(6, before.1));
    feedback.sync_mode(PlayMode::RepeatOne, anim);
    assert_eq!(feedback.mode_caption().0.mode, PlayMode::RepeatOne);
    assert_eq!(feedback.mode_caption().2, before.2, "括号不跳回起点");
    feedback.sync_mode(PlayMode::Shuffle, anim);
    assert_eq!(feedback.mode_caption().0.mode, PlayMode::Shuffle);
    assert_eq!(feedback.mode_caption().2, before.2, "反向缩宽保持当前宽度");
    feedback.tick(PlayMode::Shuffle, anim, now);
    let progressing = feedback.mode_caption();
    feedback.sync_mode(PlayMode::Shuffle, anim);
    assert_eq!(feedback.mode_caption(), progressing, "重复同步不重启动画");
    settle(&mut feedback, PlayMode::Shuffle, anim, now);
    let (caption, progress, width) = feedback.mode_caption();
    assert_eq!(caption.mode, PlayMode::Shuffle);
    assert_eq!(progress, 1000);
    assert_eq!(caption.label_opacity(3, progress), 1000);
    assert_eq!(width, 6);
    Ok(())
}
