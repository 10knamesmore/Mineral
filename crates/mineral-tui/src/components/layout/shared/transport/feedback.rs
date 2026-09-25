//! 播放栏的本地反馈生命周期：动作只唤起提示，文字内容来自已确认的播放镜像。

use std::fmt::Debug;
use std::time::{Duration, Instant};

use mineral_config::AnimationConfig;
use mineral_protocol::PlayMode;

use crate::components::layout::shared::text::display_width;
use crate::render::anim::{Transition, ease_in_out, lerp_u16, ticks16_from_ms};
use crate::runtime::action::Action;

/// 左上标题的可见内容；音量数字在绘制时读取已确认值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Heading {
    /// 日常面板标题。
    Transport,

    /// 最近调节过音量。
    Volume,
}

/// 模式按钮的文字目标，短标签的期限与整组按钮分开。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModeCaption {
    /// 服务端已确认的模式。
    pub(super) mode: PlayMode,

    /// 是否在图标后展开短标签。
    pub(super) expanded: bool,
}

impl ModeCaption {
    /// 仅影响 TUI 的短标签，不改变协议和 CLI 文案。
    pub(super) fn label(self) -> &'static str {
        match self.mode {
            PlayMode::Sequential => "seq",
            PlayMode::RepeatAll => "rep all",
            PlayMode::RepeatOne => "rep one",
            PlayMode::Shuffle => "shuf",
        }
    }

    /// 括号内完整内容的终端列宽。
    fn width(self) -> u16 {
        display_width(self.mode.glyph())
            + if self.expanded {
                1 + display_width(self.label())
            } else {
                0
            }
    }

    /// 前半程从左到右启动各列淡入，后半程依次达到满亮度；短标签均为 ASCII。
    pub(super) fn label_opacity(self, column: u16, progress: u16) -> u16 {
        let delay = column * 500 / (display_width(self.label()) - 1);
        ease_in_out(progress.saturating_sub(delay).saturating_mul(2).min(1000))
    }
}

/// 模式图标即时反映确认态，文字逐列显现，括号独立向新宽度移动。
#[derive(Clone, Debug)]
struct ModeFeedback {
    /// 最新确认模式和当前文字提示期限决定的展开状态。
    caption: ModeCaption,
    /// 整段文字的线性显现进度，绘制时按列错开淡入。
    reveal: Transition,
    /// 括号宽度与文字显现独立推进，连按时从当前列宽接续。
    width: CaptionWidth,
}

impl ModeFeedback {
    /// 以确认模式的紧凑图标初始化，不展开文字提示。
    fn new(mode: PlayMode, anim: &AnimationConfig) -> Self {
        let caption = ModeCaption {
            mode,
            expanded: false,
        };
        Self {
            caption,
            reveal: Transition::new(ticks16_from_ms(
                *anim.transport().mode_reveal_ms(),
                *anim.frame_tick_ms(),
            )),
            width: CaptionWidth::new(caption.width(), anim),
        }
    }

    /// 新确认值立即替换内容；重复同步不重启显现，收起立即清除标签。
    fn set_caption(&mut self, caption: ModeCaption, anim: &AnimationConfig) {
        if self.caption == caption {
            return;
        }
        mineral_log::debug!(target: "tui::transport", from = ?self.caption, to = ?caption,
            interrupted = !self.reveal.settled(), "transport mode reveal changed");
        self.caption = caption;
        if caption.expanded {
            self.reveal = Transition::expanding(ticks16_from_ms(
                *anim.transport().mode_reveal_ms(),
                *anim.frame_tick_ms(),
            ));
        }
        self.width.target(caption.width(), anim);
    }

    /// 每帧推进一次文字显现与括号宽度，绘制不消费进度。
    fn tick(&mut self) {
        let moving = !self.reveal.settled();
        self.reveal.tick();
        self.width.motion.tick();
        if moving && self.reveal.settled() {
            mineral_log::debug!(target: "tui::transport", mode = ?self.caption.mode,
                "transport mode reveal finished");
        }
    }

    /// 配置热更只调整后续步长，保留显现进度和当前宽度。
    fn retempo(&mut self, anim: &AnimationConfig) {
        self.reveal.retempo(ticks16_from_ms(
            *anim.transport().mode_reveal_ms(),
            *anim.frame_tick_ms(),
        ));
        self.width.motion.retempo(ticks16_from_ms(
            *anim.transport().mode_resize_ms(),
            *anim.frame_tick_ms(),
        ));
    }
}

/// 旧文字先淡出，再切换到最新目标淡入；目标反向时保持当前亮度。
#[derive(Clone, Debug)]
struct TextFade<T> {
    /// 当前仍在屏幕上的内容。
    displayed: T,

    /// 最新目标，不排队保存已被后续操作取代的内容。
    target: T,

    /// 文字亮度，切换内容只发生在零亮度处。
    opacity: Transition,
}

impl<T: Copy + Eq + Debug> TextFade<T> {
    /// 初始内容直接可见，无启动动画。
    fn new(content: T, anim: &AnimationConfig) -> Self {
        let mut opacity = Transition::collapsing(ticks16_from_ms(
            *anim.transport().volume_fade_in_ms(),
            *anim.frame_tick_ms(),
        ));
        opacity.enter();
        Self {
            displayed: content,
            target: content,
            opacity,
        }
    }

    /// 从当前亮度转向最新内容；同一目标不会重置在途动画。
    fn target(&mut self, target: T, anim: &AnimationConfig) {
        if self.target == target {
            return;
        }
        mineral_log::debug!(target: "tui::transport", from = ?self.displayed, to = ?target,
            opacity = self.opacity.raw(), interrupted = !self.opacity.settled(),
            "transport text target changed");
        self.target = target;
        if self.displayed == target {
            self.opacity.enter();
        } else {
            self.opacity.leave();
        }
        self.retempo(anim);
    }

    /// 每拍推进一次，零亮度处交接内容。
    fn tick(&mut self, anim: &AnimationConfig) {
        let moving = !self.opacity.settled();
        self.opacity.tick();
        if self.opacity.at_min() && self.displayed != self.target {
            self.displayed = self.target;
            self.opacity.enter();
            self.retempo(anim);
        } else if moving && self.opacity.settled() {
            mineral_log::debug!(target: "tui::transport", content = ?self.displayed,
                "transport text transition finished");
        }
    }

    /// 只替换当前方向的步长，保留内容、目标和亮度。
    fn retempo(&mut self, anim: &AnimationConfig) {
        let ms = if self.opacity.leaving() {
            *anim.transport().volume_fade_out_ms()
        } else {
            *anim.transport().volume_fade_in_ms()
        };
        self.opacity
            .retempo(ticks16_from_ms(ms, *anim.frame_tick_ms()));
    }
}

/// 模式括号内容宽度，单位为千分之一列；中断后从当前宽度接续。
#[derive(Clone, Debug)]
struct CaptionWidth {
    /// 本段过渡起点。
    from: u16,

    /// 最新内容宽度。
    target: u16,

    /// 起点到目标的过渡。
    motion: Transition,
}

impl CaptionWidth {
    /// 紧凑图标的初始宽度。
    fn new(width: u16, anim: &AnimationConfig) -> Self {
        Self {
            from: width * 1000,
            target: width * 1000,
            motion: Transition::new(ticks16_from_ms(
                *anim.transport().mode_resize_ms(),
                *anim.frame_tick_ms(),
            )),
        }
    }

    /// 当前宽度保留亚列精度，最后由绘制取整。
    fn current(&self) -> u16 {
        lerp_u16(self.from, self.target, self.motion.eased_in_out())
    }

    /// 改变内容宽度；先采当前值，避免连按时括号跳回旧端点。
    fn target(&mut self, width: u16, anim: &AnimationConfig) {
        let target = width * 1000;
        if target == self.target {
            return;
        }
        self.from = self.current();
        self.target = target;
        self.motion = Transition::expanding(ticks16_from_ms(
            *anim.transport().mode_resize_ms(),
            *anim.frame_tick_ms(),
        ));
    }
}

/// 与播放栏同生命周期的反馈状态，由 AppState 持有，输入与 tick 显式更新。
/// 三个期限仅由对应动作刷新；后端重复同步、绘制、resize 和页面形变均不续期。
#[derive(Clone, Debug)]
pub(crate) struct TransportFeedback {
    /// 音量提示到期时刻，未触发或已到期为 None。
    volume_until: Option<Instant>,

    /// 模式短标签到期时刻。
    mode_until: Option<Instant>,

    /// 整组按钮到期时刻。
    controls_until: Option<Instant>,

    /// 左上标题的内容交接。
    heading: TextFade<Heading>,

    /// 模式文字的逐列显现与括号伸缩。
    mode: ModeFeedback,

    /// 整组底部按钮相对边框的显隐。
    controls: Transition,
}

impl TransportFeedback {
    /// 初始按钮隐藏，确认的模式仅用于准备下次唤起的内容。
    pub(crate) fn new(mode: PlayMode, anim: &AnimationConfig) -> Self {
        Self {
            volume_until: None,
            mode_until: None,
            controls_until: None,
            heading: TextFade::new(Heading::Transport, anim),
            mode: ModeFeedback::new(mode, anim),
            controls: Transition::new(ticks16_from_ms(
                *anim.transport().controls_fade_ms(),
                *anim.frame_tick_ms(),
            )),
        }
    }

    /// 只在动作实际进入执行器后调用；不修改音量、播放模式或播放状态。
    pub(crate) fn on_action(
        &mut self,
        action: Action,
        mode: PlayMode,
        anim: &AnimationConfig,
        now: Instant,
    ) {
        let cfg = anim.transport();
        match action {
            Action::NudgeVolume(_) => {
                self.volume_until =
                    Some(now + Duration::from_millis(u64::from(*cfg.volume_hold_ms())));
                self.heading.target(Heading::Volume, anim);
            }
            Action::PrevOrRestart
            | Action::TogglePlayPause
            | Action::NextSong
            | Action::CyclePlayMode => {
                self.controls_until =
                    Some(now + Duration::from_millis(u64::from(*cfg.controls_hold_ms())));
                self.controls.enter();
                if action == Action::CyclePlayMode {
                    self.mode_until =
                        Some(now + Duration::from_millis(u64::from(*cfg.mode_hold_ms())));
                }
                self.sync_mode(mode, anim);
            }
            _ => return,
        }
        mineral_log::debug!(target: "tui::transport", ?action, controls = self.controls.raw(),
            "transport feedback triggered");
    }

    /// 同步已确认模式，仅更新内容目标，不刷新任何期限。
    pub(crate) fn sync_mode(&mut self, mode: PlayMode, anim: &AnimationConfig) {
        let caption = ModeCaption {
            mode,
            expanded: self.mode_until.is_some(),
        };
        self.mode.set_caption(caption, anim);
    }

    /// 显式 tick：到期转向退场，然后各动画只推进一次。
    pub(crate) fn tick(&mut self, mode: PlayMode, anim: &AnimationConfig, now: Instant) {
        if expire(&mut self.volume_until, now, "volume") {
            self.heading.target(Heading::Transport, anim);
        }
        expire(&mut self.mode_until, now, "mode label");
        if expire(&mut self.controls_until, now, "controls") {
            self.controls.leave();
        }
        self.sync_mode(mode, anim);
        self.heading.tick(anim);
        self.mode.tick();
        let moving = !self.controls.settled();
        self.controls.tick();
        if moving && self.controls.settled() {
            mineral_log::debug!(target: "tui::transport", visible = self.controls.at_max(),
                "transport controls transition finished");
        }
    }

    /// 配置应用入口调用，所有在途相位及已建立期限保持原样。
    pub(crate) fn retempo(&mut self, anim: &AnimationConfig) {
        self.heading.retempo(anim);
        self.mode.retempo(anim);
        self.controls.retempo(ticks16_from_ms(
            *anim.transport().controls_fade_ms(),
            *anim.frame_tick_ms(),
        ));
    }

    /// 整组按钮的显现程度，千分比，绘制可重复读取。
    pub(crate) fn controls_opacity(&self) -> u16 {
        self.controls.eased_in_out()
    }

    /// 当前标题与文字亮度。
    pub(super) fn heading(&self) -> (Heading, u16) {
        (self.heading.displayed, self.heading.opacity.eased_in_out())
    }

    /// 当前模式、文字显现进度及括号内宽度；过渡中至少保留完整图标。
    pub(super) fn mode_caption(&self) -> (ModeCaption, u16, u16) {
        (
            self.mode.caption,
            self.mode.reveal.raw(),
            ((self.mode.width.current() + 500) / 1000)
                .max(display_width(self.mode.caption.mode.glyph())),
        )
    }
}

/// 到期只消费一次，便于区分首次退场与后续空闲 tick。
fn expire(until: &mut Option<Instant>, now: Instant, label: &str) -> bool {
    if until.is_some_and(|deadline| now >= deadline) {
        *until = None;
        mineral_log::debug!(target: "tui::transport", feedback = label, "transport feedback expired");
        true
    } else {
        false
    }
}

#[cfg(test)]
#[path = "feedback_tests.rs"]
mod tests;
