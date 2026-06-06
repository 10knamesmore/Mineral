//! topbar 通知层:多条 [`crate::components::toast::toast::Toast`] 的**堆叠管理器** + 通用纯文本内容 [`TextItem`]。
//!
//! 对外三个语义入口:[`Notifications::flash`](一次性、TTL 自动退场)、
//! [`Notifications::flash_keyed`](同 key 顶替不堆叠 + 续 TTL,高频更新类提示用)与
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
        /// 退场时刻(推送时 = `now + flash_ttl`)。
        deadline: Instant,

        /// 顶替键:同 key 的后续 [`Notifications::flash_keyed`] 替换本条内容并
        /// 续 TTL;`None` 为匿名 flash,不参与顶替。
        key: Option<String>,
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

    /// 一次性通知的展示时长(配置 `toast.flash_ttl_secs`)。
    flash_ttl: Duration,

    /// toast 进 / 出场动画 tick 数(配置 `animation.toast_anim_ticks`)。
    anim_ticks: u16,
}

impl Notifications {
    /// 新建空通知管理器。
    ///
    /// # Params:
    ///   - `flash_ttl_secs`: 一次性通知展示秒数(配置 `toast.flash_ttl_secs`)
    ///   - `toast_anim_ticks`: toast 进 / 出场动画 tick 数(配置 `animation.toast_anim_ticks`)
    pub(crate) fn new(flash_ttl_secs: u64, toast_anim_ticks: u16) -> Self {
        Self {
            entries: Vec::new(),
            flash_ttl: Duration::from_secs(flash_ttl_secs),
            anim_ticks: toast_anim_ticks,
        }
    }

    /// 推一条一次性通知(`deadline = now + flash_ttl`)。多次调用 = 多条堆叠。
    ///
    /// # Params:
    ///   - `item`: 要显示的内容
    pub(crate) fn flash(&mut self, item: Box<dyn ToastItem>) {
        self.flash_for(item, /*ttl*/ None);
    }

    /// 同 [`Self::flash`],但可覆盖展示时长(脚本 toast 的 `ttl_secs`)。
    ///
    /// # Params:
    ///   - `item`: 要显示的内容
    ///   - `ttl`: 展示时长;`None` 用配置默认(`toast.flash_ttl_secs`)
    pub(crate) fn flash_for(&mut self, item: Box<dyn ToastItem>, ttl: Option<Duration>) {
        let mut toast = Toast::new(self.anim_ticks);
        toast.set(Some(item));
        self.entries.push(Entry {
            toast,
            life: Life::Flash {
                deadline: Instant::now() + ttl.unwrap_or(self.flash_ttl),
                key: None,
            },
        });
    }

    /// 推一条**带顶替键**的一次性通知:同 key 的存活条目被替换内容并续 TTL
    /// (退场中也复活),不存在则新建。高频更新类提示(脚本音量观察等)用它防刷屏。
    ///
    /// # Params:
    ///   - `key`: 顶替键
    ///   - `item`: 要显示的内容
    ///   - `ttl`: 展示时长;`None` 用配置默认(`toast.flash_ttl_secs`)
    pub(crate) fn flash_keyed_for(
        &mut self,
        key: String,
        item: Box<dyn ToastItem>,
        ttl: Option<Duration>,
    ) {
        self.flash_keyed_at(key, item, ttl, Instant::now());
    }

    /// [`Self::flash_keyed_for`] 实现体;`now` 可注入(测试确定性,与
    /// [`Self::prune_expired`] 同款手法)。
    fn flash_keyed_at(
        &mut self,
        key: String,
        item: Box<dyn ToastItem>,
        ttl: Option<Duration>,
        now: Instant,
    ) {
        let deadline = now + ttl.unwrap_or(self.flash_ttl);
        let found = self
            .entries
            .iter_mut()
            .find(|e| matches!(&e.life, Life::Flash { key: Some(k), .. } if *k == key));
        match found {
            Some(entry) => {
                entry.toast.set(Some(item));
                entry.life = Life::Flash {
                    deadline,
                    key: Some(key),
                };
            }
            None => {
                let mut toast = Toast::new(self.anim_ticks);
                toast.set(Some(item));
                self.entries.push(Entry {
                    toast,
                    life: Life::Flash {
                        deadline,
                        key: Some(key),
                    },
                });
            }
        }
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
                    let mut toast = Toast::new(self.anim_ticks);
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

    /// 当前条目数(含退场动画中的)。仅测试断言用。
    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
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
            if let Life::Flash { deadline, .. } = entry.life
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

/// 纯文本通知的语义级别 → 渲染时映射主题色(普通 `text` / 警告 `yellow` / 错误 `red`)。
#[derive(Clone, Copy)]
pub(crate) enum TextTint {
    /// 普通信息。
    Normal,

    /// 警告。
    Warn,

    /// 错误。
    Error,
}

/// 通用纯文本通知内容。
pub(crate) struct TextItem {
    /// 提示文本。
    text: String,

    /// 语义级别(决定前景色)。
    tint: TextTint,
}

/// 用一段文本构造通知内容(boxed,普通级别)。
///
/// # Params:
///   - `text`: 单行提示
///
/// # Return:
///   boxed [`ToastItem`]。
pub(crate) fn text_item(text: String) -> Box<dyn ToastItem> {
    tinted_text_item(text, TextTint::Normal)
}

/// 用一段文本 + 语义级别构造通知内容(boxed)。
///
/// toast 是单行渲染:多行输入(如带 traceback 的错误)只取首行——否则
/// 宽度按整串计算,括号被撑满全屏、内容显示成空白。
///
/// # Params:
///   - `text`: 提示文本(多行时截取首行)
///   - `tint`: 语义级别(决定前景色)
///
/// # Return:
///   boxed [`ToastItem`]。
pub(crate) fn tinted_text_item(text: String, tint: TextTint) -> Box<dyn ToastItem> {
    let text = match text.find('\n') {
        Some(idx) => text.get(..idx).unwrap_or_default().to_owned(),
        None => text,
    };
    Box::new(TextItem { text, tint })
}

impl ToastItem for TextItem {
    fn width(&self) -> u16 {
        u16::try_from(UnicodeWidthStr::width(self.text.as_str()))
            .unwrap_or(0)
            .saturating_add(2) // 左右各留一空格
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let fg = match self.tint {
            TextTint::Normal => theme.text,
            TextTint::Warn => theme.yellow,
            TextTint::Error => theme.red,
        };
        frame.render_widget(
            Paragraph::new(format!(" {} ", self.text)).style(Style::new().fg(fg).bg(theme.base)),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::{LiveSlot, Notifications, text_item};
    use crate::render::theme::Theme;

    /// 多行内容(如带 traceback 的错误)只取首行:宽度按首行算,
    /// 不被换行后的长尾撑满全屏。
    #[test]
    fn multiline_text_item_keeps_first_line_only() {
        let item = text_item("首行错误\nstack traceback:\n\t[C]: in ?".to_owned());
        let single = text_item("首行错误".to_owned());
        assert_eq!(item.width(), single.width(), "多行内容的宽度应等于首行宽度");
    }

    /// 测试对照值 = default.lua 默认(flash_ttl_secs=4 / toast_anim_ms=96 ÷ 16ms = 6 拍)。
    const FLASH_TTL: Duration = Duration::from_secs(4);

    /// 同上:toast 动画 tick 数默认。
    const ANIM_TICKS: u16 = 6;

    /// 以默认旋钮构造通知管理器。
    fn notifications() -> Notifications {
        Notifications::new(
            /*flash_ttl_secs*/ 4, /*toast_anim_ticks*/ ANIM_TICKS,
        )
    }

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
        let mut n = notifications();
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
        let mut n = notifications();
        n.set_live(LiveSlot::DOWNLOAD, Some(text_item("10%".to_owned())));
        n.set_live(LiveSlot::DOWNLOAD, Some(text_item("20%".to_owned())));
        assert_eq!(n.entries.len(), 1);
        Ok(())
    }

    /// `set_live(slot, None)` 触发退场,动画归零后条目被清理。
    #[test]
    fn set_live_none_retires_and_clears() -> color_eyre::Result<()> {
        let mut n = notifications();
        n.set_live(LiveSlot::DOWNLOAD, Some(text_item("进行中".to_owned())));
        run(&mut n, 8); // 展开
        assert_eq!(n.entries.len(), 1);
        n.set_live(LiveSlot::DOWNLOAD, /*item*/ None);
        run(&mut n, 8); // 退场归零
        assert!(n.entries.is_empty(), "退场后应被清理");
        Ok(())
    }

    /// flash_keyed:同 key 顶替不堆叠;不同 key / 匿名 flash 各自独立堆叠。
    #[test]
    fn flash_keyed_same_key_replaces_not_stacks() -> color_eyre::Result<()> {
        let mut n = notifications();
        n.flash_keyed_for(
            "vol".to_owned(),
            text_item("音量 31".to_owned()),
            /*ttl*/ None,
        );
        n.flash_keyed_for(
            "vol".to_owned(),
            text_item("音量 32".to_owned()),
            /*ttl*/ None,
        );
        n.flash_keyed_for(
            "vol".to_owned(),
            text_item("音量 33".to_owned()),
            /*ttl*/ None,
        );
        assert_eq!(n.entries.len(), 1, "同 key 应顶替为一条");

        n.flash_keyed_for(
            "mode".to_owned(),
            text_item("shuffle".to_owned()),
            /*ttl*/ None,
        );
        assert_eq!(n.entries.len(), 2, "不同 key 各自一条");

        n.flash_text("匿名提示".to_owned());
        n.flash_text("匿名提示".to_owned());
        assert_eq!(n.entries.len(), 4, "匿名 flash 不参与顶替,照常堆叠");
        Ok(())
    }

    /// per-toast TTL 覆盖:`ttl = Some(0)` 的条目 deadline 即推送时刻,
    /// 一拍后就该进退场;同时推的默认 TTL(4s)条目仍存活。
    #[test]
    fn flash_keyed_ttl_override_expires_early() -> color_eyre::Result<()> {
        let mut n = notifications();
        let t0 = Instant::now();
        n.flash_keyed_at(
            "instant".to_owned(),
            text_item("瞬间".to_owned()),
            Some(Duration::ZERO),
            t0,
        );
        n.flash_keyed_at(
            "normal".to_owned(),
            text_item("正常".to_owned()),
            /*ttl*/ None,
            t0,
        );
        // 推够拍数:ttl=0 的走完退场动画被移除,默认 TTL 的远未到期仍在。
        run(&mut n, 16);
        assert_eq!(n.entry_count(), 1, "短 ttl 应已退场,默认 ttl 仍存活");
        Ok(())
    }

    /// flash_keyed 顶替时续命:刷新把 deadline 推后,老 deadline 过点后仍存活,
    /// 新 deadline 过点后才退场。
    #[test]
    fn flash_keyed_refreshes_deadline() -> color_eyre::Result<()> {
        let mut n = notifications();
        let t0 = Instant::now();
        n.flash_keyed_at(
            "vol".to_owned(),
            text_item("音量 31".to_owned()),
            /*ttl*/ None,
            t0,
        );
        // 2 秒后顶替:deadline 应推后到 t0+2+TTL。
        n.flash_keyed_at(
            "vol".to_owned(),
            text_item("音量 32".to_owned()),
            /*ttl*/ None,
            t0 + Duration::from_secs(2),
        );
        run(&mut n, 8); // 展开
        // 老 deadline(t0+TTL)已过、新 deadline(t0+2+TTL)未到 → 应仍存活。
        n.prune_expired(t0 + FLASH_TTL + Duration::from_secs(1));
        run(&mut n, 8);
        assert_eq!(n.entries.len(), 1, "续命后老 deadline 过点不该退场");
        // 新 deadline 过点 → 退场清理。
        n.prune_expired(t0 + Duration::from_secs(2) + FLASH_TTL + Duration::from_secs(1));
        run(&mut n, 8);
        assert!(n.entries.is_empty(), "新 deadline 过点应清理");
        Ok(())
    }

    /// flash 超过 TTL 后退场,归零后条目被清理。
    #[test]
    fn flash_expires_and_clears() -> color_eyre::Result<()> {
        let mut n = notifications();
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
        let mut n = notifications();
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
