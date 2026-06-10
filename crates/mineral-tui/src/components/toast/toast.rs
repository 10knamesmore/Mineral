//! 通用 topbar 提示条:在 topbar 占一行、用 `[…]` 包住一段会进出场的内容。
//!
//! 组件只管三件事:**占位**、**横向进出场动画**、**画两侧括号**。"画什么、怎么上色" 全交给
//! 实现 [`ToastItem`] 的内容自己决定(如下载进度条按进度变色)—— 组件不持有任何业务语义,
//! 也不接收预渲染的 `String`。调用方每帧用 [`Toast::set`] 声明「现在该显示什么」。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Clear, Paragraph};

use crate::render::anim::{Transition, lerp_u16};
use crate::render::theme::Theme;

/// 一段可放进 [`Toast`] 的内容:自报宽度、自渲染。实现方决定怎么画。
pub(crate) trait ToastItem {
    /// 期望内容宽度(字符,**不含**两侧括号);决定括号区总宽与居中。
    fn width(&self) -> u16;

    /// 是否「必要」通知(错误这类不容错过的)。immersive 全屏歌词下
    /// 非必要内容一律不渲染(生命周期照常走)。默认非必要。
    fn essential(&self) -> bool {
        false
    }

    /// 把内容画进 `area`(高 1、宽 = 动画展开后的内部可用宽,可能短于 [`Self::width`],自行裁剪)。
    ///
    /// # Params:
    ///   - `frame`: 目标帧
    ///   - `area`: 内部一行区域(已扣掉两侧括号)
    ///   - `theme`: 配色
    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme);
}

/// 进度满值(千分比),与 [`crate::render::anim::Transition`] 同制。
const FULL: u32 = 1000;

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

    /// 当前内容是否「必要」通知(见 [`ToastItem::essential`];无内容视为非必要)。
    pub(crate) fn essential(&self) -> bool {
        self.item.as_ref().is_some_and(|it| it.essential())
    }

    /// 在 `bar`(topbar 一行)渲染 `[ 内容 ]`,宽度按动画横向展开 / 收起。
    ///
    /// 水平位置是**连续**的:`blend` 把 x 从「行内居中」(0)插值到「贴行右缘」
    /// (1000)——布局形变(z 切换)期间通知随之平移,不瞬移;`shrink` 在动画宽
    /// 之上再乘一个宽度因子(非必要项随形变进度横向收起,复用进出场的收缩视觉)。
    ///
    /// # Params:
    ///   - `frame`: 目标帧
    ///   - `bar`: topbar 行区域
    ///   - `theme`: 配色
    ///   - `blend`: 水平锚点插值(千分比,0 = 居中,1000 = 右贴边)
    ///   - `shrink`: 宽度因子(千分比,1000 = 不收缩,0 = 收没不画)
    pub(crate) fn render(
        &self,
        frame: &mut Frame<'_>,
        bar: Rect,
        theme: &Theme,
        blend: u16,
        shrink: u16,
    ) {
        if bar.width == 0 || bar.height == 0 || !self.anim.active() {
            return;
        }
        let Some(item) = &self.item else {
            return;
        };
        let full = item.width().saturating_add(2).min(bar.width); // `[` 内容 `]`
        let w_anim = u32::from(full) * u32::from(self.anim.eased_in_out()) / FULL;
        let w = u16::try_from(w_anim * u32::from(shrink.min(1000)) / FULL).unwrap_or(full);
        if w < 2 {
            return; // 容不下两侧括号
        }
        let y = bar.y;
        let x_center = bar.x + bar.width.saturating_sub(w) / 2;
        let x_right = bar.x + bar.width.saturating_sub(w);
        let x = lerp_u16(x_center, x_right, blend);
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
