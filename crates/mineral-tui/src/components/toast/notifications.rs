//! topbar 通知层:多条 [`crate::components::toast::toast::Toast`] 的**堆叠管理器** + 通用纯文本内容 [`TextItem`]。
//!
//! 对外只有两个语义入口:[`Notifications::flash`](一次性、TTL 自动退场)与
//! [`Notifications::set_live`](按 [`LiveSlot`] 标识的常驻进度源,显式置 `None` 才退场)。
//! 多条并存时垂直堆叠:**live 区在上、flash 区在下**,每条复用单条 [`crate::components::toast::toast::Toast`]
//! 的居中括号 + 独立进出场动画。本层不持有任何业务语义 —— 「显示什么」由调用方喂 [`ToastItem`]。

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::components::toast::toast::{Toast, ToastItem};
use crate::render::theme::Theme;

/// 一次性通知存活时长;[`Notifications::flash`] 推送的条目超过它后自动退场。
const FLASH_TTL: Duration = Duration::from_secs(4);

/// 常驻进度源标识。仿 [`mineral_model::SourceKind`] 范式:newtype + 关联常量,
/// `Copy`、强类型、**开放**(未来加源只追加常量,通知层零改)。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveSlot(&'static str);

impl LiveSlot {
    /// 下载进度源。
    pub(crate) const DOWNLOAD: Self = Self("download");
}

/// 一条通知的生命周期归属。
enum Life {
    /// 一次性:到 `deadline` 自动退场。
    Flash {
        /// 退场时刻(推送时 = `now + FLASH_TTL`)。
        deadline: Instant,
    },

    /// 常驻源:由调用方显式 [`Notifications::set_live`] 置 `None` 才退场。
    Live {
        /// 源标识。
        slot: LiveSlot,
    },
}

/// 一条通知:单条渲染单元 + 它的生命周期。
struct Entry {
    /// 复用的单条渲染单元(占位 + 进出场动画 + 内容)。
    toast: Toast,

    /// 这条的生命周期。
    life: Life,
}

/// 多条堆叠的 topbar 通知管理器。
pub(crate) struct Notifications {
    /// 当前所有通知;渲染时按 live 在上、flash 在下排布。
    entries: Vec<Entry>,
}

impl Notifications {
    /// 新建空通知管理器。
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 推一条一次性通知(`deadline = now + FLASH_TTL`)。多次调用 = 多条堆叠。
    ///
    /// # Params:
    ///   - `item`: 要显示的内容
    pub(crate) fn flash(&mut self, item: Box<dyn ToastItem>) {
        let mut toast = Toast::new();
        toast.set(Some(item));
        self.entries.push(Entry {
            toast,
            life: Life::Flash {
                deadline: Instant::now() + FLASH_TTL,
            },
        });
    }

    /// 便捷:推一条一次性纯文本通知(内部构造 [`TextItem`])。
    ///
    /// # Params:
    ///   - `text`: 单行提示
    pub(crate) fn flash_text(&mut self, text: String) {
        self.flash(text_item(text));
    }

    /// upsert / 移除一个常驻源:
    ///   - `Some(item)`:该 slot 有则刷新内容,无则新建并进场;
    ///   - `None`:触发该 slot 退场(动画收完才真正移除)。
    ///
    /// # Params:
    ///   - `slot`: 源标识
    ///   - `item`: 当前内容,或 `None` 表示该源结束
    pub(crate) fn set_live(&mut self, slot: LiveSlot, item: Option<Box<dyn ToastItem>>) {
        match self.live_entry(slot) {
            Some(entry) => entry.toast.set(item),
            None => {
                if let Some(item) = item {
                    let mut toast = Toast::new();
                    toast.set(Some(item));
                    self.entries.push(Entry {
                        toast,
                        life: Life::Live { slot },
                    });
                }
            }
        }
    }

    /// 取某 slot 的常驻条目(可变借用)。
    fn live_entry(&mut self, slot: LiveSlot) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .find(|e| matches!(e.life, Life::Live { slot: s } if s == slot))
    }

    /// 每帧推进:① 过期 flash 触发退场;② 推进每条动画;③ 移除已休眠(退场归零)的条目。
    pub(crate) fn tick(&mut self) {
        self.prune_expired(Instant::now());
        for entry in &mut self.entries {
            entry.toast.tick();
        }
        self.entries.retain(|e| !e.toast.dormant());
    }

    /// 把所有到期 flash 切入退场。
    ///
    /// # Params:
    ///   - `now`: 当前时刻(测试可注入)
    fn prune_expired(&mut self, now: Instant) {
        for entry in &mut self.entries {
            if let Life::Flash { deadline } = entry.life
                && now >= deadline
            {
                entry.toast.set(None);
            }
        }
    }

    /// 从 `bar`(topbar 锚点行)起向下垂直堆叠渲染;live 区在上、flash 区在下,
    /// 行数 clamp 到屏幕可用高度。
    ///
    /// # Params:
    ///   - `frame`: 目标帧
    ///   - `bar`: topbar 锚点行(决定起始 y、横向居中基准)
    ///   - `theme`: 配色
    pub(crate) fn render(&self, frame: &mut Frame<'_>, bar: Rect, theme: &Theme) {
        let max_rows = usize::from(frame.area().height.saturating_sub(bar.y));
        if max_rows == 0 || bar.width == 0 {
            return;
        }
        let lives = self
            .entries
            .iter()
            .filter(|e| matches!(e.life, Life::Live { .. }));
        let flashes = self
            .entries
            .iter()
            .filter(|e| matches!(e.life, Life::Flash { .. }));
        for (i, entry) in lives.chain(flashes).take(max_rows).enumerate() {
            let dy = u16::try_from(i).unwrap_or(0);
            let row = Rect::new(bar.x, bar.y.saturating_add(dy), bar.width, 1);
            entry.toast.render(frame, row, theme);
        }
    }
}

/// 通用纯文本通知内容。
pub(crate) struct TextItem {
    /// 提示文本。
    text: String,
}

/// 用一段文本构造通知内容(boxed)。
///
/// # Params:
///   - `text`: 单行提示
///
/// # Return:
///   boxed [`ToastItem`]。
pub(crate) fn text_item(text: String) -> Box<dyn ToastItem> {
    Box::new(TextItem { text })
}

impl ToastItem for TextItem {
    fn width(&self) -> u16 {
        u16::try_from(UnicodeWidthStr::width(self.text.as_str()))
            .unwrap_or(0)
            .saturating_add(2) // 左右各留一空格
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        frame.render_widget(
            Paragraph::new(format!(" {} ", self.text))
                .style(Style::new().fg(theme.text).bg(theme.base)),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::{FLASH_TTL, LiveSlot, Notifications, text_item};
    use crate::render::theme::Theme;

    /// 推进 n 帧(每帧刷新同一 live 内容,模拟持续展开)。
    fn run(n: &mut Notifications, frames: usize) {
        for _ in 0..frames {
            n.tick();
        }
    }

    /// live + flash 并存:`下载中` 在上、`出错了` 在下,各自居中括号。
    #[test]
    fn stack_live_above_flash_snapshot() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut n = Notifications::new();
        n.flash_text("出错了".to_owned());
        for _ in 0..8 {
            n.set_live(LiveSlot::DOWNLOAD, Some(text_item("下载中 62%".to_owned())));
            n.tick();
        }

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            n.render(f, area, &theme);
        })?;
        crate::test_support::assert_snap!(
            "通知堆叠:live(下载中)在上、flash(出错了)在下",
            t.backend()
        );
        Ok(())
    }

    /// 同 slot 二次 `set_live(Some)` 只刷新、不新增条目。
    #[test]
    fn set_live_same_slot_refreshes_not_duplicates() -> color_eyre::Result<()> {
        let mut n = Notifications::new();
        n.set_live(LiveSlot::DOWNLOAD, Some(text_item("10%".to_owned())));
        n.set_live(LiveSlot::DOWNLOAD, Some(text_item("20%".to_owned())));
        assert_eq!(n.entries.len(), 1);
        Ok(())
    }

    /// `set_live(slot, None)` 触发退场,动画归零后条目被清理。
    #[test]
    fn set_live_none_retires_and_clears() -> color_eyre::Result<()> {
        let mut n = Notifications::new();
        n.set_live(LiveSlot::DOWNLOAD, Some(text_item("进行中".to_owned())));
        run(&mut n, 8); // 展开
        assert_eq!(n.entries.len(), 1);
        n.set_live(LiveSlot::DOWNLOAD, /*item*/ None);
        run(&mut n, 8); // 退场归零
        assert!(n.entries.is_empty(), "退场后应被清理");
        Ok(())
    }

    /// flash 超过 TTL 后退场,归零后条目被清理。
    #[test]
    fn flash_expires_and_clears() -> color_eyre::Result<()> {
        let mut n = Notifications::new();
        n.flash_text("瞬时".to_owned());
        run(&mut n, 8); // 展开(未过期)
        assert_eq!(n.entries.len(), 1);
        n.prune_expired(Instant::now() + FLASH_TTL + Duration::from_secs(1));
        run(&mut n, 8); // 退场归零
        assert!(n.entries.is_empty(), "过期后应被清理");
        Ok(())
    }

    /// 进出场动画中途一帧:括号区由中心横向展开,尚未铺满。
    #[test]
    fn toast_midexpand_snapshot() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut n = Notifications::new();
        for _ in 0..2 {
            n.set_live(
                LiveSlot::DOWNLOAD,
                Some(text_item("下载中 62% 2.4MB/s".to_owned())),
            );
            n.tick();
        }

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            n.render(f, area, &theme);
        })?;
        crate::test_support::assert_snap!("通知进场动画中途:括号区由中心横向展开", t.backend());
        Ok(())
    }
}
