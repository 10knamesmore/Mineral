//! 控制键按压底色：短暂保持后淡出，连按重新开始。

use mineral_config::AnimationConfig;
use ratatui::style::Color;

use super::anim::{Transition, ease_in_out, ticks16_from_ms};
use super::color::lerp_color;
use super::theme::Theme;

/// 单个控制键的按压反馈，由动作触发、主循环推进。
#[derive(Clone, Debug, Default)]
pub(crate) struct ControlPress {
    /// 本次按压的剩余进度；空闲时没有动画。
    progress: Option<Transition>,
}

impl ControlPress {
    /// 立即亮起，重新开始本键的反馈。
    pub(crate) fn trigger(&mut self, anim: &AnimationConfig) {
        self.progress = Some(Transition::collapsing(ticks16_from_ms(
            *anim.controls_press_ms(),
            *anim.frame_tick_ms(),
        )));
    }

    /// 推进一帧，结束时清除按压状态。
    pub(crate) fn tick(&mut self) {
        if let Some(progress) = self.progress.as_mut() {
            progress.tick();
            if progress.at_min() {
                self.progress = None;
            }
        }
    }

    /// 配置热更保留当前相位，只调整后续速度。
    pub(crate) fn retempo(&mut self, anim: &AnimationConfig) {
        if let Some(progress) = self.progress.as_mut() {
            progress.retempo(ticks16_from_ms(
                *anim.controls_press_ms(),
                *anim.frame_tick_ms(),
            ));
        }
    }

    /// 底色强度为千分比；前四分之一保持，其余时间淡出。
    pub(crate) fn strength(&self) -> u16 {
        self.progress.map_or(0, |progress| {
            ease_in_out((progress.raw() * 4 / 3).min(1000))
        })
    }
}

/// 从实际背景混合到主题按压底色；终端默认背景按主题 base 计算。
pub(crate) fn background(sampled: Color, strength: u16, theme: &Theme) -> Color {
    let base = if matches!(sampled, Color::Rgb(..)) {
        sampled
    } else {
        theme.base
    };
    lerp_color(base, theme.surface1, u64::from(strength), 1000)
}
