//! 播放栏的本地反馈生命周期：动作只唤起提示，文字内容来自已确认的播放镜像。

use std::fmt::Debug;
use std::time::{Duration, Instant};

use mineral_config::AnimationConfig;
use mineral_protocol::PlayMode;

use crate::components::layout::shared::text::display_width;
use crate::render::anim::{Transition, ease_in_out, lerp_u16, ticks16_from_ms};
use crate::render::control_press::ControlPress;
use crate::runtime::action::Action;

/// 左上标题的可见内容；音量数字在绘制时读取已确认值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Heading {
    /// 日常面板标题。
    Transport,

    /// 最近调节过音量。
    Volume,
}

/// 接收本地按压反馈的控制键，与用户实际映射的 Action 对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ControlButton {
    /// 上一首或从头播放。
    Previous,
    /// 播放 / 暂停。
    PlayPause,
    /// 下一首。
    Next,
    /// 切换播放模式。
    Mode,
}

impl ControlButton {
    /// 非控制键动作不触发按钮反馈。
    fn from_action(action: Action) -> Option<Self> {
        match action {
            Action::PrevOrRestart => Some(Self::Previous),
            Action::TogglePlayPause => Some(Self::PlayPause),
            Action::NextSong => Some(Self::Next),
            Action::CyclePlayMode => Some(Self::Mode),
            _ => None,
        }
    }
}

/// 绘制读取的单键显现程度和按压底色强度，范围均为 0..=1000。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ButtonAppearance {
    /// 相对边框的显现程度，按中的按钮可以先于整组出现。
    pub(super) opacity: u16,

    /// 短暂底色强度；不决定播放图标或模式内容。
    pub(super) press_strength: u16,
}

/// 一个按钮的可见性与短暂按压反馈；按压结束后仍跟随整组停留期限。
#[derive(Clone, Debug)]
struct ButtonFeedback {
    /// 入场可被按键直接置为可见，退场与整组同时开始。
    visibility: Transition,

    /// 最近一次按压的底色反馈。
    press: ControlPress,
}

impl ButtonFeedback {
    /// 初始随控制组隐藏，没有按压反馈。
    fn new(anim: &AnimationConfig) -> Self {
        Self {
            visibility: Transition::new(ticks16_from_ms(
                *anim.transport().controls_fade_ms(),
                *anim.frame_tick_ms(),
            )),
            press: ControlPress::default(),
        }
    }

    /// 立即显示本键并重启短脉冲，避免反馈被整组入场遮住。
    fn press(&mut self, anim: &AnimationConfig) {
        self.visibility = Transition::collapsing(ticks16_from_ms(
            *anim.transport().controls_fade_ms(),
            *anim.frame_tick_ms(),
        ));
        self.visibility.enter();
        self.press.trigger(anim);
    }

    /// 逐帧更新，在脉冲结束时恢复没有按压的状态。
    fn tick(&mut self) {
        self.visibility.tick();
        self.press.tick();
    }

    /// 保留显隐与按压相位，只调整后续步长。
    fn retempo(&mut self, anim: &AnimationConfig) {
        self.visibility.retempo(ticks16_from_ms(
            *anim.transport().controls_fade_ms(),
            *anim.frame_tick_ms(),
        ));
        self.press.retempo(anim);
    }

    /// 脉冲前四分之一保持满强度，其余时间平滑淡出。
    fn appearance(&self) -> ButtonAppearance {
        ButtonAppearance {
            opacity: self.visibility.eased_in_out(),
            press_strength: self.press.strength(),
        }
    }
}

/// 控制组的间隔显隐与四个按钮的独立反馈，按上一首、播放、下一首、模式排列。
#[derive(Clone, Debug)]
struct ControlsFeedback {
    /// 按钮之间的空白与原边框共用的入退场进度。
    visibility: Transition,

    /// 每颗按钮可以即时响应自身按压，不中断其他按钮。
    buttons: [ButtonFeedback; 4],
}

impl ControlsFeedback {
    /// 整组与每颗按钮都从隐藏状态开始。
    fn new(anim: &AnimationConfig) -> Self {
        Self {
            visibility: Transition::new(ticks16_from_ms(
                *anim.transport().controls_fade_ms(),
                *anim.frame_tick_ms(),
            )),
            buttons: std::array::from_fn(|_| ButtonFeedback::new(anim)),
        }
    }

    /// 唤出整组，仅被按中的按钮立即出现并显示底色。
    fn press(&mut self, button: ControlButton, anim: &AnimationConfig) {
        self.visibility.enter();
        for feedback in &mut self.buttons {
            feedback.visibility.enter();
        }
        let [previous, play_pause, next, mode] = &mut self.buttons;
        match button {
            ControlButton::Previous => previous,
            ControlButton::PlayPause => play_pause,
            ControlButton::Next => next,
            ControlButton::Mode => mode,
        }
        .press(anim);
        mineral_log::debug!(target: "tui::transport", ?button, "transport button pressed");
    }

    /// 停留期限到期后，各按钮从自己的当前可见程度退场。
    fn leave(&mut self) {
        self.visibility.leave();
        for button in &mut self.buttons {
            button.visibility.leave();
        }
    }

    /// 每拍推进整组与各按钮，绘制只读取这些相位。
    fn tick(&mut self) {
        self.visibility.tick();
        for button in &mut self.buttons {
            button.tick();
        }
    }

    /// 热更同时覆盖整组显隐和各按钮的独立相位。
    fn retempo(&mut self, anim: &AnimationConfig) {
        self.visibility.retempo(ticks16_from_ms(
            *anim.transport().controls_fade_ms(),
            *anim.frame_tick_ms(),
        ));
        for button in &mut self.buttons {
            button.retempo(anim);
        }
    }

    /// 按钮身份直接映射到固定槽位，不依赖可变索引。
    fn button(&self, button: ControlButton) -> ButtonAppearance {
        let [previous, play_pause, next, mode] = &self.buttons;
        match button {
            ControlButton::Previous => previous,
            ControlButton::PlayPause => play_pause,
            ControlButton::Next => next,
            ControlButton::Mode => mode,
        }
        .appearance()
    }
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

    /// 整组底部按钮的显隐与单键按压反馈。
    controls: ControlsFeedback,

    /// Seek 操作触发的已播放时间底色。
    elapsed_press: ControlPress,
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
            controls: ControlsFeedback::new(anim),
            elapsed_press: ControlPress::default(),
        }
    }

    /// Seek 请求发出时亮起左侧时间，不唤出播放控件或修改时间数值。
    pub(crate) fn on_seek(&mut self, anim: &AnimationConfig) {
        self.elapsed_press.trigger(anim);
        mineral_log::debug!(target: "tui::transport", "seek time feedback triggered");
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
            _ => {
                let Some(button) = ControlButton::from_action(action) else {
                    return;
                };
                self.controls_until =
                    Some(now + Duration::from_millis(u64::from(*cfg.controls_hold_ms())));
                self.controls.press(button, anim);
                if button == ControlButton::Mode {
                    self.mode_until =
                        Some(now + Duration::from_millis(u64::from(*cfg.mode_hold_ms())));
                }
                self.sync_mode(mode, anim);
            }
        }
        mineral_log::debug!(target: "tui::transport", ?action, controls = self.controls.visibility.raw(),
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
        self.elapsed_press.tick();
        let moving = !self.controls.visibility.settled();
        self.controls.tick();
        if moving && self.controls.visibility.settled() {
            mineral_log::debug!(target: "tui::transport", visible = self.controls.visibility.at_max(),
                "transport controls transition finished");
        }
    }

    /// 配置应用入口调用，所有在途相位及已建立期限保持原样。
    pub(crate) fn retempo(&mut self, anim: &AnimationConfig) {
        self.heading.retempo(anim);
        self.mode.retempo(anim);
        self.controls.retempo(anim);
        self.elapsed_press.retempo(anim);
    }

    /// 左侧时间的按压底色强度。
    pub(super) fn elapsed_press_strength(&self) -> u16 {
        self.elapsed_press.strength()
    }

    /// 整组按钮的显现程度，千分比，绘制可重复读取。
    pub(crate) fn controls_opacity(&self) -> u16 {
        self.controls.visibility.eased_in_out()
    }

    /// 被按中的按钮可先于整组间隔显示。
    pub(super) fn has_visible_buttons(&self) -> bool {
        self.controls
            .buttons
            .iter()
            .any(|button| !button.visibility.at_min())
    }

    /// 返回一颗按钮的当前外观，不消费反馈状态。
    pub(super) fn button(&self, button: ControlButton) -> ButtonAppearance {
        self.controls.button(button)
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
