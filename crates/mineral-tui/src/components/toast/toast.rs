//! 通用 topbar 提示条:在 topbar 占一行、用 `[…]` 包住一段会进出场的内容。
//!
//! 组件只管三件事:**占位**、**横向进出场动画**、**画两侧括号**。"画什么、怎么上色" 全交给
//! 实现 [`ToastItem`] 的内容自己决定(如下载进度条按进度变色)—— 组件不持有任何业务语义,
//! 也不接收预渲染的 `String`。调用方每帧用 [`Toast::set`] 声明「现在该显示什么」。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Clear, Paragraph};

use crate::render::anim::Transition;
use crate::render::theme::Theme;

/// 一段可放进 [`Toast`] 的内容:自报宽度、自渲染。实现方决定怎么画。
pub(crate) trait ToastItem {
    /// 期望内容宽度(字符,**不含**两侧括号);决定括号区总宽与居中。
    fn width(&self) -> u16;

    /// 把内容画进 `area`(高 1、宽 = 动画展开后的内部可用宽,可能短于 [`Self::width`],自行裁剪)。
    ///
    /// # Params:
    ///   - `frame`: 目标帧
    ///   - `area`: 内部一行区域(已扣掉两侧括号)
    ///   - `theme`: 配色
    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme);
}

/// topbar 提示条:托管一段 [`ToastItem`] + 横向进出场动画。
pub(crate) struct Toast {
    /// 横向展开 / 收起动画(有内容时 enter、无内容时 leave)。
    anim: Transition,

    /// 当前内容;退场动画放完(归零)后才真正丢弃。
    item: Option<Box<dyn ToastItem>>,
}

impl Toast {
    /// 新建一个收起态、无内容的提示条。
    /// 新建收起态 toast。
    ///
    /// # Params:
    ///   - `anim_ticks`: 进 / 出场动画 tick 数(配置 `animation.toast_anim_ticks`)
    pub(crate) fn new(anim_ticks: u16) -> Self {
        Self {
            anim: Transition::new(anim_ticks),
            item: None,
        }
    }

    /// 声明「当前该显示什么」(`None` = 收起)。每帧调用:`Some` 刷新内容并进场、
    /// `None` 退场(动画收完才真正丢弃内容)。
    ///
    /// # Params:
    ///   - `item`: 当前内容,或 `None`
    pub(crate) fn set(&mut self, item: Option<Box<dyn ToastItem>>) {
        match item {
            Some(it) => {
                self.item = Some(it);
                self.anim.enter();
            }
            None => self.anim.leave(),
        }
    }

    /// 推进进出场动画;完全收起(归零)后丢弃残留内容。
    pub(crate) fn tick(&mut self) {
        self.anim.tick();
        if self.anim.at_min() {
            self.item = None;
        }
    }

    /// 是否彻底休眠:动画归零且无内容 —— 管理器可安全丢弃这条。
    pub(crate) fn dormant(&self) -> bool {
        !self.anim.active() && self.item.is_none()
    }

    /// 在 `bar`(topbar 一行)top-center 渲染 `[ 内容 ]`,宽度按动画横向展开 / 收起。
    ///
    /// # Params:
    ///   - `frame`: 目标帧
    ///   - `bar`: topbar 行区域(弹条贴其顶边居中)
    ///   - `theme`: 配色
    pub(crate) fn render(&self, frame: &mut Frame<'_>, bar: Rect, theme: &Theme) {
        if bar.width == 0 || bar.height == 0 || !self.anim.active() {
            return;
        }
        let Some(item) = &self.item else {
            return;
        };
        let full = item.width().saturating_add(2).min(bar.width); // `[` 内容 `]`
        let w = u16::try_from(u32::from(full) * u32::from(self.anim.eased_in_out()) / 1000)
            .unwrap_or(full);
        if w < 2 {
            return; // 容不下两侧括号
        }
        let y = bar.y;
        let x = bar.x + bar.width.saturating_sub(w) / 2;
        frame.render_widget(Clear, Rect::new(x, y, w, 1));
        let bracket = Style::new().fg(theme.accent).bg(theme.base);
        frame.render_widget(Paragraph::new("[").style(bracket), Rect::new(x, y, 1, 1));
        frame.render_widget(
            Paragraph::new("]").style(bracket),
            Rect::new(x + w - 1, y, 1, 1),
        );
        // 内部区域交给内容自渲染(宽度随动画变化,内容自行裁剪)。
        item.render(frame, Rect::new(x + 1, y, w - 2, 1), theme);
    }
}
