//! topbar 通知层:多条 [`crate::components::toast::toast::Toast`] 的**堆叠管理器** + 通用纯文本内容 [`TextItem`]。
//!
//! 对外四个语义入口:[`Notifications::flash`](一次性、TTL 自动退场)、
//! [`Notifications::flash_keyed_for`](同 key 顶替不堆叠 + 续 TTL,高频更新类提示用)、
//! [`Notifications::set_live`](按 [`LiveSlot`] 标识的常驻进度源,显式置 `None` 才退场)
//! 与 [`Notifications::push_card`](多行驻留卡片,[`Notifications::dismiss_card`] 显式关)。
//! 单行条目垂直堆叠:**live 区在上、flash 区在下**,卡片接在其后;渲染按沉浸进度
//! (千分比)连续分布 —— 0 为常规模式顶部居中向下,1000 为 immersive 右上角第 2 行
//! 起且**只画必要项**([`ToastItem::essential`] 的单行 + 全部卡片),中间值锚点插值
//! (z 切换跟随布局形变),非必要项随进度横向收起但生命周期照常走。
//! 本层不持有任何业务语义 —— 「显示什么」由调用方喂 [`ToastItem`] / 卡片数据。

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::components::toast::card::{Card, CardMotion, CardTtl};
use crate::components::toast::toast::{Toast, ToastItem};
use crate::render::anim::lerp_u16;
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
    /// 当前所有单行通知;渲染时按 live 在上、flash 在下排布。
    entries: Vec<Entry>,

    /// 当前所有驻留卡片(推入序),渲染接在单行条目之后。
    cards: Vec<Card>,

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
            cards: Vec::new(),
            flash_ttl: Duration::from_secs(flash_ttl_secs),
            anim_ticks: toast_anim_ticks,
        }
    }

    /// 配置热更:就地重设默认展示时长与进出场拍数,**存活通知不动**
    /// (在场的按旧节奏放完,新来的用新参数)。
    ///
    /// # Params:
    ///   - `flash_ttl_secs`: 一次性通知展示秒数(配置 `toast.flash_ttl_secs`)
    ///   - `toast_anim_ticks`: toast 进 / 出场动画 tick 数
    pub(crate) fn retempo(&mut self, flash_ttl_secs: u64, toast_anim_ticks: u16) {
        self.flash_ttl = Duration::from_secs(flash_ttl_secs);
        self.anim_ticks = toast_anim_ticks;
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

    /// 便捷:推一条一次性纯文本通知(内部构造 [`TextItem`])。仅测试用。
    ///
    /// # Params:
    ///   - `text`: 单行提示
    #[cfg(test)]
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

    /// 推一张多行卡片。`id: Some` 顶替同 id 的存活卡(退场中复活),
    /// `None` 堆叠。`ttl: Some` 到时自动退场(边框暗色随剩余时间自左上
    /// 向右下蔓延);`None` 驻留,[`Self::dismiss_card`] 显式关才退场。
    ///
    /// # Params:
    ///   - `tint`: 语义级别(边框 / 标题色)
    ///   - `title`: 标题 spans(画进边框;空不画)
    ///   - `body`: 行 / 行内 spans(调用方已按 `\n` 拆好行)
    ///   - `id`: 顶替键
    ///   - `ttl`: 展示时长;`None` 驻留
    pub(crate) fn push_card(
        &mut self,
        tint: TextTint,
        title: Vec<mineral_protocol::TextSpan>,
        body: Vec<Vec<mineral_protocol::TextSpan>>,
        id: Option<String>,
        ttl: Option<Duration>,
    ) {
        let life = ttl.map(|t| CardTtl {
            deadline: Instant::now() + t,
            total: t,
        });
        if let Some(key) = &id
            && let Some(card) = self
                .cards
                .iter_mut()
                .find(|c| c.id().is_some_and(|cid| cid == key))
        {
            card.refresh(tint, title, body, life);
            return;
        }
        self.cards
            .push(Card::new(tint, title, body, id, self.anim_ticks, life));
    }

    /// 关最早一张还没在退场的卡片(连按逐条关)。
    ///
    /// # Return:
    ///   被关卡片的顶替键(调用方据此识别「哪张被关了」,如版本要点卡关闭即记录);
    ///   无卡可关、或该卡无 id,都是 `None`。
    pub(crate) fn dismiss_card(&mut self) -> Option<&str> {
        match self.cards.iter_mut().find(|c| !c.leaving()) {
            Some(card) => {
                card.dismiss();
                card.id()
            }
            None => None,
        }
    }

    /// 撤下指定 id 的卡片(进退场动画;无此 id / 已在退场则空操作)。
    /// 供「问题已解决」场景主动收卡,如干净重载撤掉上次的配置警告卡。
    ///
    /// # Params:
    ///   - `id`: 顶替键
    pub(crate) fn dismiss_card_by_id(&mut self, id: &str) {
        if let Some(card) = self
            .cards
            .iter_mut()
            .find(|c| c.id().is_some_and(|cid| cid == id) && !c.leaving())
        {
            card.dismiss();
        }
    }

    /// 当前条目数(含退场动画中的)。仅测试断言用。
    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// 当前卡片数(含退场动画中的)。仅测试断言用。
    #[cfg(test)]
    pub(crate) fn card_count(&self) -> usize {
        self.cards.len()
    }

    /// 是否存在指定 id 的**存活**(未退场)卡片。仅测试断言用。
    #[cfg(test)]
    pub(crate) fn has_live_card(&self, id: &str) -> bool {
        self.cards
            .iter()
            .any(|c| c.id().is_some_and(|cid| cid == id) && !c.leaving())
    }

    /// 每帧推进:① 过期 flash 触发退场;② 推进每条动画;③ 移除已休眠(退场归零)的条目。
    pub(crate) fn tick(&mut self) {
        self.prune_expired(Instant::now());
        for entry in &mut self.entries {
            entry.toast.tick();
        }
        self.entries.retain(|e| !e.toast.dormant());
        for card in &mut self.cards {
            card.tick();
        }
        self.cards.retain(|c| !c.dormant());
    }

    /// 把所有到期 flash / 带 TTL 的到期卡片切入退场。
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
        for card in &mut self.cards {
            if card.expired(now) && !card.leaving() {
                card.dismiss();
            }
        }
    }

    /// 从 `bar`(topbar 锚点行)起向下垂直堆叠渲染:单行区(live 在上、flash 在下)
    /// → 卡片区,总高 clamp 到屏幕可用高度。
    ///
    /// `immersive` 是**连续**的形变进度(千分比):0 = 常规(顶部居中、全员可见)、
    /// 1000 = 沉浸全屏(右上第 2 行起、只留必要项)。中间值时每个元素的锚点在
    /// 两套槽位之间插值 —— z 切换期间通知跟着布局一起飞,不瞬移;非必要项
    /// ([`ToastItem::essential`] 为假的单行)随进度横向收起(复用进出场的收缩
    /// 视觉),不画但生命周期照常走。
    ///
    /// # Params:
    ///   - `frame`: 目标帧
    ///   - `bar`: topbar 锚点行(决定起始 y 与水平基准)
    ///   - `theme`: 配色
    ///   - `immersive`: 沉浸进度(千分比,喂 `fullscreen_pos.eased_in_out()`)
    ///   - `close_hint`: 卡片底边关闭键提示(如 `x`)
    pub(crate) fn render(
        &self,
        frame: &mut Frame<'_>,
        bar: Rect,
        theme: &Theme,
        immersive: u16,
        close_hint: &str,
    ) {
        if bar.width == 0 {
            return;
        }
        let p = immersive.min(1000);
        // 沉浸端锚:顶行让给歌词边框的档位指示(tr·[t]),从第 2 行起、右缘留 1 列。
        let band_w = lerp_u16(bar.width, bar.width.saturating_sub(1), p);
        let screen_bottom = frame.area().height;
        // 两套槽位各自的 y 游标:常规流(含非必要项)与沉浸流(只含必要项),
        // 每个元素的实际 y 在自己的两个游标之间按 p 插值。
        let mut y_n = bar.y;
        let mut y_i = bar.y.saturating_add(1);

        let lives = self
            .entries
            .iter()
            .filter(|e| matches!(e.life, Life::Live { .. }));
        let flashes = self
            .entries
            .iter()
            .filter(|e| matches!(e.life, Life::Flash { .. }));
        for entry in lives.chain(flashes) {
            let essential = entry.toast.essential();
            let (y, blend, shrink) = if essential {
                (lerp_u16(y_n, y_i, p), p, 1000)
            } else {
                // 非必要项:钉在常规槽位,随进度横向收起;到端点收没。
                (y_n, 0, 1000_u16.saturating_sub(p))
            };
            if y < screen_bottom && shrink > 0 {
                let row = Rect::new(bar.x, y, band_w, 1);
                entry.toast.render(frame, row, theme, blend, shrink);
            }
            y_n = y_n.saturating_add(1);
            if essential {
                y_i = y_i.saturating_add(1);
            }
        }

        let motion = if p < 500 {
            CardMotion::ExpandDown
        } else {
            CardMotion::SlideInRight
        };
        let now = Instant::now();
        for card in &self.cards {
            let h = card.height();
            let y = lerp_u16(y_n, y_i, p);
            if y.saturating_add(h) > screen_bottom {
                return; // 放不下整张就停(不画半张)
            }
            let w = card.width(close_hint).min(band_w);
            let x_center = bar.x + band_w.saturating_sub(w) / 2;
            let x_right = bar.x + band_w.saturating_sub(w);
            let x = lerp_u16(x_center, x_right, p);
            card.render(frame, Rect::new(x, y, w, h), motion, close_hint, theme, now);
            y_n = y_n.saturating_add(h);
            y_i = y_i.saturating_add(h);
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

/// 通用文本通知内容(行内 spans,样式缺省落 tint 色)。
pub(crate) struct TextItem {
    /// 行内 spans(构造时已截首行;flash 是单行胶囊,忽略 span 的 `align`)。
    spans: Vec<mineral_protocol::TextSpan>,

    /// 语义级别(决定缺省前景色与 essential)。
    tint: TextTint,
}

/// 用一段文本构造通知内容(boxed,普通级别)。仅测试用。
///
/// # Params:
///   - `text`: 单行提示
///
/// # Return:
///   boxed [`ToastItem`]。
#[cfg(test)]
pub(crate) fn text_item(text: String) -> Box<dyn ToastItem> {
    tinted_text_item(text, TextTint::Normal)
}

/// 用一段文本 + 语义级别构造通知内容(boxed,单 plain span)。
///
/// # Params:
///   - `text`: 提示文本(多行时截取首行)
///   - `tint`: 语义级别(决定缺省前景色)
///
/// # Return:
///   boxed [`ToastItem`]。
pub(crate) fn tinted_text_item(text: String, tint: TextTint) -> Box<dyn ToastItem> {
    tinted_spans_item(vec![mineral_protocol::TextSpan::plain(text)], tint)
}

/// 用一行 spans + 语义级别构造通知内容(boxed)。
///
/// toast 是单行渲染:多行输入(如带 traceback 的错误)只取首行——否则
/// 宽度按整串计算,括号被撑满全屏、内容显示成空白。
///
/// # Params:
///   - `spans`: 行内 spans(首个内嵌 `\n` 之后的内容被截掉)
///   - `tint`: 语义级别(决定缺省前景色)
///
/// # Return:
///   boxed [`ToastItem`]。
pub(crate) fn tinted_spans_item(
    spans: Vec<mineral_protocol::TextSpan>,
    tint: TextTint,
) -> Box<dyn ToastItem> {
    Box::new(TextItem {
        spans: first_line(spans),
        tint,
    })
}

/// 截到首个换行为止:跨 span 扫描,命中 `\n` 的 span 截断文本并丢弃其后所有 span。
fn first_line(spans: Vec<mineral_protocol::TextSpan>) -> Vec<mineral_protocol::TextSpan> {
    let mut out = Vec::<mineral_protocol::TextSpan>::new();
    for mut span in spans {
        match span.text.find('\n') {
            Some(idx) => {
                span.text.truncate(idx);
                if !span.text.is_empty() {
                    out.push(span);
                }
                break;
            }
            None => out.push(span),
        }
    }
    out
}

impl TextItem {
    /// 级别 → 缺省前景色。
    fn tint_color(&self, theme: &Theme) -> ratatui::style::Color {
        match self.tint {
            TextTint::Normal => theme.text,
            TextTint::Warn => theme.yellow,
            TextTint::Error => theme.red,
        }
    }
}

impl ToastItem for TextItem {
    fn width(&self) -> u16 {
        self.spans
            .iter()
            .map(|s| u16::try_from(UnicodeWidthStr::width(s.text.as_str())).unwrap_or(u16::MAX))
            .fold(0_u16, u16::saturating_add)
            .saturating_add(2) // 左右各留一空格
    }

    fn essential(&self) -> bool {
        matches!(self.tint, TextTint::Error)
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let fg = self.tint_color(theme);
        let mut line = vec![ratatui::text::Span::raw(" ")];
        line.extend(self.spans.iter().map(|s| {
            ratatui::text::Span::styled(
                s.text.as_str(),
                crate::components::toast::card::span_style(s, fg, theme),
            )
        }));
        line.push(ratatui::text::Span::raw(" "));
        frame.render_widget(
            Paragraph::new(ratatui::text::Line::from(line))
                .style(Style::new().fg(fg).bg(theme.base)),
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
    use crate::components::toast::card::plain_line;
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

    /// 纯文本卡片 body 简写。
    fn body(lines: &[&str]) -> Vec<Vec<mineral_protocol::TextSpan>> {
        crate::components::toast::card::plain_body(lines.iter().map(|s| (*s).to_owned()))
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
            n.render(f, area, &theme, /*immersive*/ 0, "x");
        })?;
        crate::test_support::assert_snap!(
            "通知堆叠:live(下载中)在上、flash(出错了)在下",
            t.backend()
        );
        Ok(())
    }

    /// 卡片 id 顶替:同 id 替换为一张、不同 id / 无 id 各自堆叠。
    #[test]
    fn push_card_same_id_replaces_not_stacks() -> color_eyre::Result<()> {
        use super::TextTint;
        let mut n = notifications();
        n.push_card(
            TextTint::Warn,
            plain_line("要点"),
            body(&["a"]),
            Some("release".to_owned()),
            /*ttl*/ None,
        );
        n.push_card(
            TextTint::Warn,
            plain_line("要点v2"),
            body(&["b"]),
            Some("release".to_owned()),
            /*ttl*/ None,
        );
        assert_eq!(n.cards.len(), 1, "同 id 应顶替");
        n.push_card(
            TextTint::Normal,
            plain_line("另一张"),
            body(&["c"]),
            /*id*/ None,
            /*ttl*/ None,
        );
        assert_eq!(n.cards.len(), 2, "无 id 应堆叠");
        Ok(())
    }

    /// 带 TTL 的卡片到点自动退场,驻留卡不受影响。
    #[test]
    fn card_with_ttl_expires_sticky_stays() -> color_eyre::Result<()> {
        use super::TextTint;
        let mut n = notifications();
        n.push_card(
            TextTint::Normal,
            plain_line("瞬态"),
            body(&["a"]),
            /*id*/ None,
            Some(Duration::ZERO),
        );
        n.push_card(
            TextTint::Normal,
            plain_line("驻留"),
            body(&["b"]),
            Some("stay".to_owned()),
            /*ttl*/ None,
        );
        run(&mut n, 16); // ttl=0 的过点 → 退场归零被清;驻留卡仍在
        assert_eq!(n.card_count(), 1, "到期卡应被清理");
        assert!(n.has_live_card("stay"), "驻留卡不受 TTL 清理影响");
        Ok(())
    }

    /// dismiss 逐条关(最早一张先关)并返回其 id;全部退场归零后被清理。
    #[test]
    fn dismiss_card_oldest_first_then_clears() -> color_eyre::Result<()> {
        use super::TextTint;
        let mut n = notifications();
        n.push_card(
            TextTint::Warn,
            plain_line("first"),
            body(&["a"]),
            Some("one".to_owned()),
            /*ttl*/ None,
        );
        n.push_card(
            TextTint::Error,
            plain_line("second"),
            body(&["b"]),
            /*id*/ None,
            /*ttl*/ None,
        );
        run(&mut n, 8); // 展开
        assert_eq!(n.dismiss_card(), Some("one"), "最早的一张先关,返回其 id");
        assert_eq!(n.dismiss_card(), None, "第二张无 id,关掉返回 None");
        assert_eq!(n.dismiss_card(), None, "没有可关的了");
        run(&mut n, 8); // 退场归零
        assert!(n.cards.is_empty(), "退场后应被清理");
        Ok(())
    }

    /// 常规模式:单行 flash 居中在上,多行卡片接在其下同样居中。
    #[test]
    fn cards_stack_below_flash_snapshot() -> color_eyre::Result<()> {
        use super::TextTint;
        let theme = Theme::default();
        let mut n = notifications();
        n.flash_text("音量 32".to_owned());
        n.push_card(
            TextTint::Warn,
            plain_line("v0.9.0 要点"),
            body(&["新增配置 toast.position", "旧键 search.style 改名"]),
            /*id*/ None,
            /*ttl*/ None,
        );
        run(&mut n, 8); // 全部展开
        let mut t = Terminal::new(TestBackend::new(60, 8))?;
        t.draw(|f| {
            let area = f.area();
            n.render(f, area, &theme, /*immersive*/ 0, "x");
        })?;
        crate::test_support::assert_snap!(
            "常规模式:flash 居中在上,驻留卡片接在其下居中",
            t.backend()
        );
        Ok(())
    }

    /// immersive:casual(live 下载 / 普通 flash)不画,essential(Error flash + 卡片)
    /// 右贴边、从第 2 行起;顶行留空给歌词边框档位指示。
    #[test]
    fn immersive_essential_only_snapshot() -> color_eyre::Result<()> {
        use super::{TextTint, tinted_text_item};
        let theme = Theme::default();
        let mut n = notifications();
        n.flash_text("普通提示不该出现".to_owned());
        n.flash(tinted_text_item("播放失败".to_owned(), TextTint::Error));
        n.push_card(
            TextTint::Error,
            plain_line("配置重载失败"),
            body(&["config.lua:17 字段 audio.volume"]),
            /*id*/ None,
            /*ttl*/ None,
        );
        for _ in 0..8 {
            n.set_live(LiveSlot::DOWNLOAD, Some(text_item("下载中 62%".to_owned())));
            n.tick();
        }
        let mut t = Terminal::new(TestBackend::new(60, 8))?;
        t.draw(|f| {
            let area = f.area();
            n.render(f, area, &theme, /*immersive*/ 1000, "x");
        })?;
        crate::test_support::assert_snap!(
            "immersive:仅 Error flash 与卡片,右贴边、第 2 行起;下载/普通提示被抑制",
            t.backend()
        );
        Ok(())
    }

    /// z 切换中点(immersive=500):卡片锚点落在「顶部居中」与「右上」之间
    /// (x、y 都插值),非必要 flash 宽度收半 —— 锁住"跟随布局飞、不瞬移"。
    #[test]
    fn z_transition_midpoint_interpolates_anchors() -> color_eyre::Result<()> {
        use super::TextTint;
        let theme = Theme::default();
        let mut n = notifications();
        n.flash_text("普通提示收缩中".to_owned());
        n.push_card(
            TextTint::Normal,
            plain_line("迁移"),
            body(&["body"]),
            /*id*/ None,
            /*ttl*/ None,
        );
        run(&mut n, 8); // 全部展开
        let mut t = Terminal::new(TestBackend::new(60, 8))?;
        t.draw(|f| {
            let area = f.area();
            n.render(f, area, &theme, /*immersive*/ 500, "x");
        })?;
        crate::test_support::assert_snap!(
            "z 切换中点:卡片在居中与右上之间,非必要 flash 收缩半宽",
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
            n.render(f, area, &theme, /*immersive*/ 0, "x");
        })?;
        crate::test_support::assert_snap!("通知进场动画中途:括号区由中心横向展开", t.backend());
        Ok(())
    }
}
