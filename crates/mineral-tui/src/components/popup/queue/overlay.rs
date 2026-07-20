//! 浮动 queue 面板:展示当前播放队列,vim 风格导航 + Enter 播放。

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table, Widget};

use super::columns::{QueueColumns, TITLE_COL};
use super::footer::remaining_label;
use super::row::{RowDecor, build_row};
use crate::components::layout::shared::marquee::{MarqueeCtx, resolve_column_widths, row_marquee};
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block, dock_full_rect,
};
use crate::render::color::lerp_color;
use crate::render::theme::{Theme, resolve_source_color};
use crate::runtime::action::{Action, SelectionMove};
use crate::runtime::marquee::Slot;
use crate::runtime::scroll;
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::state::{AppState, OverlayReveal};

/// 浮动 queue 浮层。
///
/// 只持有 UI-local 光标 + 视口滚动(`list`,永不被 server snapshot 覆盖,仅 clamp 防越界)
/// 与本地 `/` 模糊过滤态(`search`);队列曲目是后端权威态,渲染 / 导航时从 [`AppState`] 读。
///
/// `/` 过滤只重排**显示**:光标 `list` 索引的是过滤视图(按匹配分降序),经
/// [`Self::visible`] 映射回队列真实下标;底层 `player.queue`(播放顺序)分毫不动。
pub(crate) struct QueueOverlay {
    /// 光标 + 视口滚动态(UI-local;走通用 [`ScrollList`])。**索引过滤视图**,非队列 raw 下标。
    pub(super) list: ScrollList,

    /// 本地 `/` 模糊过滤态(查询串 + 输入态 + matcher);复用通用「本地模糊过滤域」。
    /// `deep_cache` 对队列恒空、不触碰。装箱是因 [`SearchState`](crate::runtime::state::SearchState)
    /// 内含 nucleo matcher + 多份缓存体量大,直接嵌入会让 `OverlayKind` 各变体尺寸悬殊。
    pub(super) search: Box<crate::runtime::state::SearchState>,
}

impl QueueOverlay {
    /// 新建:光标 + 视口直接定位到 `sel`(打开浮层时通常传在播歌下标),不从队首长程滑过来。
    pub(crate) fn new(sel: usize) -> Self {
        Self {
            list: ScrollList::at(sel),
            search: Box::new(crate::runtime::state::SearchState::new()),
        }
    }

    /// 把光标钳到 `[0, len-1]`(队列变短后防越界);空队列归 0。
    pub(crate) fn clamp(&mut self, len: usize) {
        self.list.clamp(len);
    }

    /// 当前光标行(过滤视图位;集成测试断言用)。脚本 ctx 采集要队列真实下标走
    /// [`Self::raw_cursor`],不用这个视图位。
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> usize {
        self.list.sel()
    }

    /// 选中行在屏幕上的矩形(供其上叠 `y` 复制菜单贴行下方弹)。停靠几何与渲染同源
    /// ([`dock_full_rect`]),内区去边框后:表头占 1 行,选中行 = 内区 y + 1 + (光标 − 视口
    /// offset)。`offset` 走只读 `Frozen` 快照,平移途中 `pin_cursor` 钳边与渲染端一致。
    pub(crate) fn row_anchor(&self, ctx: &AppState) -> Rect {
        let full = dock_full_rect(ctx.frame_area.get(), ctx);
        // base_block 是 Borders::ALL,内区四周各去 1。
        let inner = Rect::new(
            full.x.saturating_add(1),
            full.y.saturating_add(1),
            full.width.saturating_sub(2),
            full.height.saturating_sub(2),
        );
        let len = ctx.player.queue.len();
        let viewport = usize::from(inner.height.saturating_sub(1));
        let offset = self.list.offset(len, viewport, ScrollMotion::Frozen);
        let pinned = scroll::viewport::pin_cursor(self.list.sel(), offset, viewport);
        let dy = u16::try_from(pinned.saturating_sub(offset)).unwrap_or(0);
        Rect::new(
            inner.x,
            // +1 表头(无内层边框)。
            inner.y.saturating_add(1).saturating_add(dy),
            inner.width,
            1,
        )
    }

    /// 过滤命中为空时的占位:内区垂直中点居中一行暗色提示,替代空表(空表读成
    /// 「队列为空」,而此处是「过滤没命中」)。
    fn render_no_matches(&self, buf: &mut Buffer, inner: Rect, theme: &Theme) {
        if inner.height == 0 {
            return;
        }
        let msg = format!("no match for /{}", self.search.query());
        let line = Line::from(Span::styled(msg, Style::new().fg(theme.overlay))).centered();
        let row = Rect::new(
            inner.x,
            inner.y.saturating_add(inner.height / 2),
            inner.width,
            1,
        );
        Paragraph::new(line).render(row, buf);
    }
}

impl Overlay for QueueOverlay {
    fn chrome(&self) -> Chrome {
        Chrome {
            pct_w: 60,
            pct_h: 70,
            min_w: 40,
            min_h: 12,
            max_w: 96,
            max_h: 32,
            animated: true,
            dock: true,
            anchor: None,
            align: None,
        }
    }

    fn block(&self, ctx: &AppState, theme: &Theme, focused: bool) -> Block<'static> {
        let border_color = if focused {
            theme.accent
        } else {
            theme.surface1
        };
        let inner_w = dock_full_rect(ctx.frame_area.get(), ctx).width;
        // 顶栏:` queue ` + `/query` 输入片段(与浏览页同位,输入框在上不在底栏)。
        let mut title = vec![Span::styled(" queue ", Style::new().fg(theme.subtext))];
        title.extend(self.search_input(theme));
        base_block(theme)
            .border_style(Style::new().fg(border_color))
            .title(Line::from(title))
            // 底栏左下:过滤态 `命中位 / 命中数`,否则 `n / total`。
            .title_bottom(
                Line::from(self.position_bottom(ctx)).style(Style::new().fg(theme.overlay)),
            )
            .title_bottom(
                Line::from(remaining_label(ctx, inner_w))
                    .right_aligned()
                    .style(Style::new().fg(theme.overlay)),
            )
    }

    fn render_content(&self, buf: &mut Buffer, inner: Rect, ctx: &AppState, theme: &Theme) {
        // ▶ 标记按 server 的在播位置锚点定位(queue_current_index,下标优先),
        // 不用歌曲身份匹配——队列含重复曲时身份会把所有副本一起点亮。
        let current_idx = ctx.queue_current_index();
        // 列规格:文本档位按浮层内宽选(窄浮层退到「歌本身」),序号列宽按队列规模选
        // (≤999 首 3 宽,超过 4 宽,避免 4 位下标被定宽截断)。
        let cols = QueueColumns::resolve(inner.width, ctx.player.queue.len());
        // 序号无条件染该行歌曲的源色(零列宽成本地表示来源):同源队列整列同色即
        // 该队列来源,混源队列则逐行不同。
        let header = Row::new(cols.header_cells())
            .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

        let widths = cols.widths();
        // 表格选中行的 fade 实际会被 row_highlight_style 整行 fg 盖掉(刻意保留整行
        // accent,见 MarqueeCtx::fade_to 注);fade_to 仍按其底色给,不误导插值方向。
        let marquee_ctx = MarqueeCtx::new(ctx, theme, /*fade_to*/ theme.surface0);
        // highlight_symbol "▌ " 恒占 2 列;title 是第 2 列(# / ♥ 之后)。下标必须跟着
        // 列序走——取错列会让 marquee 按那一列的宽度去裁标题(取到 ♥ 列就只剩 2 格)。
        let title_w = resolve_column_widths(inner.width, &widths, 2)
            .get(TITLE_COL)
            .copied()
            .unwrap_or(0);
        // 过滤视图:按匹配分降序的队列真实下标;无过滤词恒等 `0..len`。空命中时画占位。
        let visible = self.visible(ctx);
        if visible.is_empty() && self.is_filtering() {
            self.render_no_matches(buf, inner, theme);
            return;
        }
        let sel = self.list.sel();
        let rows: Vec<Row<'_>> = visible
            .iter()
            .enumerate()
            .filter_map(|(view_i, &raw_i)| {
                let s = ctx.player.queue.get(raw_i)?;
                let decor = RowDecor {
                    // ▶ / # 按队列真实下标,不随过滤重排漂移。
                    is_current: current_idx == Some(raw_i),
                    loved: ctx.is_liked(s),
                    index_fg: resolve_source_color(theme, ctx.cfg.sources(), s.source()),
                    marquee: row_marquee(view_i == sel, &marquee_ctx, Slot::QueueSelected, title_w),
                    hits: self.row_hits(s),
                };
                Some(build_row(raw_i, s, theme, cols, decor))
            })
            .collect();

        // 焦点让给压在上面的菜单时,选中高亮按其揭开进度淡向底色——两层同时全亮会读成
        // 「两处都在等输入」。用同一个进度插值,交接与菜单的淡入严格同拍。
        let yielded = ctx.overlay_reveal.get().yielded();
        let (num, denom) = (u64::from(yielded), u64::from(OverlayReveal::FULL));
        let highlight_bg = lerp_color(theme.surface0, theme.base, num, denom);
        let highlight_fg = lerp_color(theme.accent, theme.subtext, num, denom);
        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(
                Style::new()
                    .bg(highlight_bg)
                    .fg(highlight_fg)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▌ ");

        // 视口行数 = 内区高 - 表头(边框归浮层 chrome);offset 跨帧持久 + 缓动平移。
        // 行数按过滤视图长度(而非队列全长),视口 / offset 才与实际画的行数对齐。
        let viewport = usize::from(inner.height.saturating_sub(1));
        render_scroll_table(
            buf,
            inner,
            table,
            &self.list,
            visible.len(),
            viewport,
            ScrollMotion::Advancing {
                scrolloff: ctx.scrolloff(),
                glide_ticks: ctx.list_glide_ticks(),
            },
        );
    }

    fn on_key(&mut self, key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        // `/` 输入态:吞键进过滤词(文本编辑),优先于一切动作与半穿透。
        if self.is_typing() {
            return self.on_search_key(key);
        }
        // 非输入态:导航 / 激活 / 关闭全走 on_action(跟随键位重映射与 behavior 步长);
        // 未映射裸键半穿透给全局(播放控制族,白名单在 App::passes_overlay)。
        OverlayResponse::Pass
    }

    fn on_action(&mut self, action: Action, ctx: &AppState) -> Option<OverlayResponse> {
        // `/` 输入态:所有键让给 on_key 做文本编辑(裸 KeyCode 分派),动作层一律不认。
        if self.is_typing() {
            return None;
        }
        // 过滤视图长度决定导航范围;光标索引视图位,操作前经 `visible` 映射回队列真实下标。
        let visible = self.visible(ctx);
        self.list.clamp(visible.len());
        let len = visible.len();
        let sel = self.list.sel();
        let raw = visible.get(sel).copied();
        match action {
            Action::MoveSelection(mv) => {
                self.list.move_by(mv, len);
                Some(OverlayResponse::Consumed)
            }
            // `<C-d>` 族:视口目标与光标同移 n 行(vim 语义);光标边界由 page 钳,
            // 视口上界由渲染端统一钳。
            Action::Scroll(step) => {
                let delta = scroll::viewport::step_delta(step, ctx.cfg.tui().behavior());
                self.list.page(delta, len, ctx.list_glide_ticks());
                Some(OverlayResponse::Consumed)
            }
            // `/`:进入模糊过滤输入态。
            Action::EnterSearch => {
                self.begin_search();
                Some(OverlayResponse::Consumed)
            }
            // back:过滤生效时先清词退出过滤(留在浮层),否则关闭本浮层。
            Action::BackOrClearSearch => {
                if self.is_filtering() {
                    self.clear_filter(ctx);
                    Some(OverlayResponse::Consumed)
                } else {
                    Some(OverlayResponse::Do(OverlayAction::CloseTop))
                }
            }
            // open_queue(toggle)/ quit 收敛为关闭本浮层。
            Action::OpenQueue | Action::OpenQuitConfirm => {
                Some(OverlayResponse::Do(OverlayAction::CloseTop))
            }
            Action::ActivateSelection => Some(raw.map_or(OverlayResponse::Consumed, |i| {
                OverlayResponse::Do(OverlayAction::PlayQueueIndex(i))
            })),
            // `y`:为当前光标行弹复制菜单(贴行下方、叠在 queue 之上);queue 只读复制,不改队列。
            Action::OpenCopyMenu => Some(raw.map_or(OverlayResponse::Consumed, |i| {
                OverlayResponse::Do(OverlayAction::CopyQueueIndex {
                    idx: i,
                    anchor: self.row_anchor(ctx),
                })
            })),
            // 操作菜单:为当前光标行弹队列操作菜单(贴行下方、叠在 queue 之上)。
            Action::OpenActionMenu => Some(raw.map_or(OverlayResponse::Consumed, |i| {
                OverlayResponse::Do(OverlayAction::QueueActionMenu {
                    idx: i,
                    anchor: self.row_anchor(ctx),
                })
            })),
            // 收藏:浮层自持光标,不走 browse 页那条「按 View 取选中曲」的路径。
            Action::ToggleLoveSelection => Some(raw.map_or(OverlayResponse::Consumed, |i| {
                OverlayResponse::Do(OverlayAction::ToggleLoveQueueIndex(i))
            })),
            // 下载光标行。
            Action::DownloadSelection => Some(raw.map_or(OverlayResponse::Consumed, |i| {
                OverlayResponse::Do(OverlayAction::DownloadQueueIndex(i))
            })),
            // 上下移动条目:过滤态屏蔽——视图按分重排后「上一格」对应队列非相邻位,
            // 移动语义混乱;无过滤时视图位即队列真实位,光标跟着歌走预移一格,端点不动。
            Action::ReorderSelection(mv) => {
                if self.is_filtering() {
                    return Some(OverlayResponse::Consumed);
                }
                let at = sel;
                let moved = match mv {
                    SelectionMove::Down(_) if at.saturating_add(1) < len => at.saturating_add(1),
                    SelectionMove::Up(_) if at > 0 => at.saturating_sub(1),
                    _ => at,
                };
                if moved == at {
                    return Some(OverlayResponse::Consumed);
                }
                self.list.move_by(mv, len);
                Some(OverlayResponse::Do(OverlayAction::ReorderQueueIndex {
                    idx: at,
                    down: matches!(mv, SelectionMove::Down(_)),
                }))
            }
            // 跳回在播条目:找到在播歌在过滤视图中的位置移光标(被过滤掉则不动)。只移光标
            // 不碰视口——视口交给渲染端 Advancing 缓动滚过去,故是「滚动到那里」而非瞬移。
            Action::JumpToCurrent => {
                if let Some(cur) = ctx.queue_current_index()
                    && let Some(view_i) = visible.iter().position(|&r| r == cur)
                {
                    self.list.set_sel(view_i);
                }
                Some(OverlayResponse::Consumed)
            }
            // 其余(播放控制族等)不认 → 回落 on_key(Pass 半穿透)。
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::QueueOverlay;
    use crate::components::popup::component::{
        Overlay, OverlayAction, OverlayResponse, render_overlay,
    };
    use crate::render::theme::Theme;
    use crate::runtime::action::{Action, SelectionMove};
    use crate::runtime::state::AppState;
    use crate::test_support::endserenading;

    /// 造一个填了队列的 [`AppState`],并把在播歌设为下标 `current`(None 表示无在播)。
    fn ctx_with_queue(len: usize, current: Option<usize>) -> color_eyre::Result<AppState> {
        let mut s = AppState::test_default()?;
        let queue = endserenading(len);
        s.playback.track = current.and_then(|i| queue.get(i).cloned());
        // 游标是 ▶ 标记的下标依据(server 在播锚点的镜像)。
        s.player.cursor = mineral_protocol::PlayCursor::InQueue(current.unwrap_or(0));
        s.player.queue = queue;
        Ok(s)
    }

    /// 数缓冲区里某符号出现的次数(lint 安全,经 `buf.cell` 取值不裸索引)。
    fn count_symbol(backend: &TestBackend, sym: &str) -> usize {
        let buf = backend.buffer();
        let area = buf.area;
        let mut n = 0;
        for y in 0..area.height {
            for x in 0..area.width {
                if buf.cell((x, y)).is_some_and(|c| c.symbol() == sym) {
                    n += 1;
                }
            }
        }
        n
    }

    /// 回归:队列含重复曲、在播锚点落在**第二个**副本时,
    /// 只有那一行标 `▶`——历史 bug 按歌曲身份匹配会把两个副本一起点亮(count==2)。
    #[test]
    fn queue_duplicate_marks_only_anchor_row() -> color_eyre::Result<()> {
        use mineral_test::song;
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = vec![song("a"), song("b"), song("a"), song("b")];
        ctx.player.cursor = mineral_protocol::PlayCursor::InQueue(2); // 第二个 a 正在播
        ctx.playback.track = Some(song("a"));
        let overlay = QueueOverlay::new(2);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        assert_eq!(
            count_symbol(t.backend(), "▶"),
            1,
            "重复曲只应有一行标在播,不能两个副本一起亮"
        );
        Ok(())
    }

    /// 混源 queue:`#` 序号按该行歌曲的源色着色;未配置色的源(local)退中立兜底。
    #[test]
    fn queue_mixed_source_tints_index() -> color_eyre::Result<()> {
        use mineral_model::SourceKind;

        use crate::render::theme::resolve_source_color;

        let theme = Theme::default();
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = crate::test_support::mixed_source_songs();
        let overlay = QueueOverlay::new(0);
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &theme);
        })?;
        let fg_of = |ch: &str| -> Option<ratatui::style::Color> {
            let buf = t.backend().buffer();
            let area = buf.area;
            (0..area.height).find_map(|y| {
                (0..area.width)
                    .find_map(|x| buf.cell((x, y)).filter(|c| c.symbol() == ch).map(|c| c.fg))
            })
        };
        // 行 0(netease)是光标行:row_highlight_style 盖掉 cell 级前景(accent),
        // 序号源色暂不可见——与设计一致,故断言非选中的行 1 / 2。
        assert_eq!(fg_of("0"), Some(theme.accent), "光标行序号被高亮前景覆盖");
        let bilibili = resolve_source_color(&theme, ctx.cfg.sources(), SourceKind::BILIBILI);
        assert_eq!(fg_of("1"), Some(bilibili), "bilibili 行序号染 bilibili 色");
        assert_eq!(fg_of("2"), Some(theme.subtext), "local 未配置色,退中立兜底");
        Ok(())
    }

    /// 同源 queue:序号也无条件染该源色(整列同色),不再退中立灰。
    #[test]
    fn queue_single_source_also_tints_index() -> color_eyre::Result<()> {
        use mineral_model::SourceKind;

        use crate::render::theme::resolve_source_color;

        let theme = Theme::default();
        let ctx = ctx_with_queue(3, /*current*/ None)?;
        let overlay = QueueOverlay::new(0);
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &theme);
        })?;
        let buf = t.backend().buffer();
        let area = buf.area;
        let fg = (0..area.height).find_map(|y| {
            (0..area.width)
                .find_map(|x| buf.cell((x, y)).filter(|c| c.symbol() == "1").map(|c| c.fg))
        });
        // endserenading 全 netease:非选中行序号染 netease 源色。
        let netease = resolve_source_color(&theme, ctx.cfg.sources(), SourceKind::NETEASE);
        assert_eq!(fg, Some(netease), "同源 queue 序号也染该源色");
        Ok(())
    }

    /// 空 queue,完全展开。
    #[test]
    fn queue_empty_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let ctx = AppState::test_default()?;
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(
                f,
                f.area(),
                &overlay,
                /*scale*/ 1000,
                /*focused*/ true,
                &ctx,
                &Theme::default(),
            );
        })?;
        crate::test_support::assert_snap!("队列浮层:空队列", t.backend());
        Ok(())
    }

    /// EndSerenading 前 3 曲 + 当前在播标记(下标 1)+ 聚焦,完全展开。
    /// backend=100(默认停靠占宽 36% → 浮层 36 → 内区 34)落 Song 档。
    #[test]
    fn queue_with_items_focused_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        let ctx = ctx_with_queue(3, Some(1))?;
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:EndSerenading 前 3 曲,当前在播(▶)+ 聚焦",
            t.backend()
        );
        Ok(())
    }

    /// 队列浮层底栏:左下 `n / total`,右下 `剩余曲目 · 时长 → 预计播完钟点`。
    /// 设够宽的 frame_area 让右下不退化;playing + 固定 now 让 ends 钟点稳定可锁。
    #[test]
    fn queue_footer_end_clock_snapshot() -> color_eyre::Result<()> {
        use chrono::TimeZone;

        let mut t = Terminal::new(TestBackend::new(160, 24))?;
        let mut ctx = ctx_with_queue(4, Some(0))?;
        ctx.frame_area
            .set(ratatui::layout::Rect::new(0, 0, 160, 24));
        ctx.playback.playing = true;
        ctx.playback.position_ms = 0;
        ctx.now.set(
            chrono::Local
                .with_ymd_and_hms(2026, 7, 20, 14, 44, 0)
                .single()
                .ok_or_else(|| color_eyre::eyre::eyre!("构造固定钟点失败"))?,
        );
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "队列浮层底栏:左下 n/total,右下 剩余曲目·时长→预计播完钟点",
            t.backend()
        );
        Ok(())
    }

    /// 队列浮层收藏列:已收藏行画实心 ♥,未收藏行留空(不画空心 ♡,免满屏噪音)。
    #[test]
    fn queue_love_column_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        let mut ctx = ctx_with_queue(3, Some(1))?;
        // 标记第 0、2 首为已收藏(第 1 首留未收藏,同屏对照)。
        if let Some(song) = ctx.player.queue.first().cloned() {
            ctx.toggle_loved_local(&song);
        }
        if let Some(song) = ctx.player.queue.get(2).cloned() {
            ctx.toggle_loved_local(&song);
        }
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:♥ 列——第 0/2 首已收藏画实心,第 1 首(在播)未收藏留空",
            t.backend()
        );
        Ok(())
    }

    /// 队列浮层歌名带别名:标题后缀暗色 ` (alias)`(与曲目表 / 播放栏 / 搜索结果一致)。
    /// 锁住「新增渲染面漏挂 alias 后缀」的回归。
    #[test]
    fn queue_alias_suffix_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        let mut ctx = ctx_with_queue(3, Some(1))?;
        // 真实样本:迷星叫 / 别名 Mayoiuta。
        if let Some(s) = ctx.player.queue.first_mut() {
            *s = mineral_test::aliased_song();
        }
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:歌名带译名别名,标题后缀暗色 (alias)",
            t.backend()
        );
        Ok(())
    }

    /// 小终端(backend=60,默认停靠占宽 36% → 浮层 21 → 内区 19)落 Song 档:
    /// 只剩 # / title / len,artist 省去。
    #[test]
    fn queue_narrow_song_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let ctx = ctx_with_queue(3, Some(1))?;
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:窄浮层退到 Song 档(只剩歌名,无 artist)",
            t.backend()
        );
        Ok(())
    }

    /// 中宽终端(backend=140,默认停靠占宽 36% → 浮层 50 → 内区 48)落 Full 档:
    /// 放得下 artist 但塞不进 album——锁住「中档不硬塞 album 挤瘦 title/artist」。
    #[test]
    fn queue_mid_full_no_album_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(140, 20))?;
        let ctx = ctx_with_queue(3, Some(1))?;
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:中宽保持 Full 档(有 artist,无 album)",
            t.backend()
        );
        Ok(())
    }

    /// 宽终端(backend=170,默认停靠占宽 36% → 浮层 61 → 内区 59)落 Wide 档:
    /// # / title / artist / album / len。三首歌 album 全有值(短英文 / 长英文 / CJK
    /// 混排),验证 album 列有内容时多文本列渲染不串列;其余 fixture 的 album 多为空,
    /// 覆盖不到这条路径。
    #[test]
    fn queue_wide_album_snapshot() -> color_eyre::Result<()> {
        use mineral_test::{song, with_album, with_artist, with_duration, with_name};
        let make = |name: &str, artist: &str, album: &str| {
            with_album(
                with_artist(with_duration(with_name(song(name), name), 210_000), artist),
                album,
            )
        };
        let mut t = Terminal::new(TestBackend::new(170, 24))?;
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = vec![
            make("Bones", "HONNE", "no song"),
            make("Location Unknown", "HONNE", "Warm on a Cold Night"),
            make("无", "草东没有派对", "丑奴儿"),
        ];
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:宽浮层 Wide 档 album 列有内容(短英文/长英文/CJK)",
            t.backend()
        );
        Ok(())
    }

    /// 超千首队列:序号列自适应到 4 宽,4 位下标(1000+)完整渲染不被截断。
    /// 回归历史 bug——固定 3 宽会把 `1234` 截成 `123`。光标定在 1234 使该行进视口。
    #[test]
    fn queue_wide_index_no_truncation_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(100, 16))?;
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = (0..1300)
            .map(|i| mineral_test::song(&format!("q{i}")))
            .collect();
        let overlay = QueueOverlay::new(1234);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:超千首序号列 4 宽,4 位下标完整不截断",
            t.backend()
        );
        Ok(())
    }

    /// 贴边滑入半程(scale=500):真面板右侧列(含右边框)贴左缘滑入,表格内容
    /// 随前沿平移可见,左边框尚未进场。
    #[test]
    fn queue_mid_animation_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let ctx = ctx_with_queue(3, None)?;
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(
                f,
                f.area(),
                &overlay,
                /*scale*/ 500,
                true,
                &ctx,
                &Theme::default(),
            );
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:贴边滑入半程(scale=500)内容随前沿平移",
            t.backend()
        );
        Ok(())
    }

    /// 滑入 scale=505:前沿(右)落在非整 cell,用左八分块 1/8 平滑过渡,
    /// 验证滑入不一格一格跳。
    #[test]
    fn queue_h_grow_smooth_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        let ctx = ctx_with_queue(3, None)?;
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(
                f,
                f.area(),
                &overlay,
                /*scale*/ 505,
                true,
                &ctx,
                &Theme::default(),
            );
        })?;
        crate::test_support::assert_snap!(
            "队列浮层:贴边滑入(scale=505)前沿 1/8 八分块平滑",
            t.backend()
        );
        Ok(())
    }

    /// 全屏布局下停靠右半(`fullscreen=true`),完全展开:浮层贴右缘、避开左侧封面。
    #[test]
    fn queue_fullscreen_dock_right_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        let mut ctx = ctx_with_queue(3, Some(1))?;
        ctx.browse.fullscreen.set(true);
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default());
        })?;
        crate::test_support::assert_snap!("队列浮层:全屏布局停靠右半(避开左侧封面)", t.backend());
        Ok(())
    }

    /// `<C-d>` 族滚动动作:翻页档移 `page_scroll_rows`、单行档移 `line_scroll_rows`
    /// (步长随默认配置算,调默认值不该改这条测试),越界钳首末行,均被 `Consumed`。
    #[test]
    fn scroll_action_pages_queue_cursor() -> color_eyre::Result<()> {
        use crate::runtime::action::ScrollStep;
        // EndSerenading fixture 只有 10 首,翻页步长不止 10 行,队列要更长。
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = (0..100)
            .map(|i| mineral_test::song(&format!("q{i}")))
            .collect();
        let page = *ctx.cfg.tui().behavior().page_scroll_rows();
        let line = *ctx.cfg.tui().behavior().line_scroll_rows();
        let mut o = QueueOverlay::new(0);
        assert!(matches!(
            o.on_action(Action::Scroll(ScrollStep::PageDown), &ctx),
            Some(OverlayResponse::Consumed)
        ));
        assert_eq!(o.cursor(), page, "翻页档下移 page_scroll_rows");
        o.on_action(Action::Scroll(ScrollStep::LineDown), &ctx);
        assert_eq!(o.cursor(), page + line, "单行档下移 line_scroll_rows");
        o.on_action(Action::Scroll(ScrollStep::PageUp), &ctx);
        o.on_action(Action::Scroll(ScrollStep::PageUp), &ctx);
        assert_eq!(o.cursor(), 0, "上滚越界钳到首行");
        Ok(())
    }

    /// `y`(OpenCopyMenu)产出 CopyQueueIndex(当前光标);`o`(OpenActionMenu)被显式吞掉
    /// (Consumed)——queue 只读复制、无操作语义。
    #[test]
    fn copy_menu_action_and_action_menu_swallowed() -> color_eyre::Result<()> {
        let ctx = ctx_with_queue(4, None)?;
        let mut o = QueueOverlay::new(2);
        let resp = o.on_action(Action::OpenCopyMenu, &ctx);
        let Some(OverlayResponse::Do(OverlayAction::CopyQueueIndex { idx, .. })) = resp else {
            color_eyre::eyre::bail!("y 应产出 CopyQueueIndex");
        };
        assert_eq!(idx, 2, "复制作用于当前光标行");
        let resp = o.on_action(Action::OpenActionMenu, &ctx);
        let Some(OverlayResponse::Do(OverlayAction::QueueActionMenu { idx, .. })) = resp else {
            color_eyre::eyre::bail!("o 应产出 QueueActionMenu");
        };
        assert_eq!(idx, 2, "操作同样作用于当前光标行");
        Ok(())
    }

    /// 高亮交接:上层菜单揭开到一半时,queue 的选中行底色应插值到 surface0 与 base 之间
    /// ——既不是全亮(读成两处都在等输入),也不是全灭(读成 queue 已经关了)。
    #[test]
    fn selection_highlight_fades_as_the_layer_above_opens() -> color_eyre::Result<()> {
        use crate::render::color::lerp_color;
        use crate::runtime::state::OverlayReveal;

        let theme = Theme::default();
        let ctx = ctx_with_queue(4, None)?;
        let full = u64::from(OverlayReveal::FULL);
        let half = lerp_color(theme.surface0, theme.base, full / 2, full);
        assert_ne!(half, theme.surface0, "半程不应仍是全亮底色");
        assert_ne!(half, theme.base, "半程不应已经淡到底色");

        // 无上层时不让渡:高亮保持全亮。
        ctx.overlay_reveal.set(OverlayReveal::default());
        assert_eq!(ctx.overlay_reveal.get().yielded(), 0);
        // 上层完全展开时全让:高亮到底色。
        ctx.overlay_reveal.set(OverlayReveal {
            own: OverlayReveal::FULL,
            above: OverlayReveal::FULL,
        });
        assert_eq!(
            lerp_color(
                theme.surface0,
                theme.base,
                u64::from(ctx.overlay_reveal.get().yielded()),
                full
            ),
            theme.base
        );
        Ok(())
    }

    /// 收藏 / 下载 / 跳回在播都以浮层私有光标为准,不走 browse 页的「当前 View 选中曲」。
    #[test]
    fn love_download_and_jump_use_the_overlay_cursor() -> color_eyre::Result<()> {
        let ctx = ctx_with_queue(6, /*current*/ Some(4))?;
        let mut o = QueueOverlay::new(1);
        assert!(matches!(
            o.on_action(Action::ToggleLoveSelection, &ctx),
            Some(OverlayResponse::Do(OverlayAction::ToggleLoveQueueIndex(1)))
        ));
        assert!(matches!(
            o.on_action(Action::DownloadSelection, &ctx),
            Some(OverlayResponse::Do(OverlayAction::DownloadQueueIndex(1)))
        ));
        assert!(matches!(
            o.on_action(Action::JumpToCurrent, &ctx),
            Some(OverlayResponse::Consumed)
        ));
        assert_eq!(o.cursor(), 4, "跳回在播条目");
        Ok(())
    }

    /// 跳回在播是「滚动过去」而非瞬移:光标即刻落在在播行,但视口目标不被 snap——
    /// 留给渲染端的 Advancing 缓动滚过去。回退成 `place`/`at` 会让视口瞬跳,本测试拦它。
    #[test]
    fn jump_to_current_scrolls_instead_of_snapping() -> color_eyre::Result<()> {
        let ctx = ctx_with_queue(10, /*current*/ Some(9))?;
        let mut o = QueueOverlay::new(0);
        assert_eq!(
            o.list.offset(
                10,
                /*viewport*/ 4,
                crate::runtime::scroll::list::ScrollMotion::Frozen
            ),
            0,
            "打开时视口在顶"
        );
        o.on_action(Action::JumpToCurrent, &ctx);
        assert_eq!(o.cursor(), 9, "光标即刻落在在播行");
        assert_eq!(
            o.list.offset(
                10,
                /*viewport*/ 4,
                crate::runtime::scroll::list::ScrollMotion::Frozen
            ),
            0,
            "视口目标未被 snap(仍在顶),靠缓动滚过去"
        );
        Ok(())
    }

    /// 移动条目:光标跟着歌走(预移一格),端点不动且不发请求。
    #[test]
    fn reorder_follows_the_song_and_stops_at_edges() -> color_eyre::Result<()> {
        let ctx = ctx_with_queue(3, None)?;
        let mut o = QueueOverlay::new(0);
        // 首项上移 = 端点,不动也不发请求。
        assert!(matches!(
            o.on_action(Action::ReorderSelection(SelectionMove::Up(1)), &ctx),
            Some(OverlayResponse::Consumed)
        ));
        assert_eq!(o.cursor(), 0, "端点不环绕");

        let resp = o.on_action(Action::ReorderSelection(SelectionMove::Down(1)), &ctx);
        let Some(OverlayResponse::Do(OverlayAction::ReorderQueueIndex { idx, down })) = resp else {
            color_eyre::eyre::bail!("下移应产出 ReorderQueueIndex");
        };
        assert_eq!((idx, down), (0, true), "带原下标与方向");
        assert_eq!(o.cursor(), 1, "光标跟着歌下移一格");
        Ok(())
    }

    /// row_anchor:无滚动时选中行落在停靠内区表头之下第 `sel` 行(去外框 1 + 表头 1）。
    #[test]
    fn row_anchor_maps_selection_within_dock() -> color_eyre::Result<()> {
        use crate::components::popup::component::dock_full_rect;
        let ctx = ctx_with_queue(5, None)?;
        ctx.frame_area
            .set(ratatui::layout::Rect::new(0, 0, 100, 30));
        let o = QueueOverlay::new(2);
        let full = dock_full_rect(ctx.frame_area.get(), &ctx);
        let anchor = o.row_anchor(&ctx);
        assert_eq!(anchor.x, full.x + 1, "锚点 x 在内区(去左边框)");
        assert_eq!(anchor.y, full.y + 2 + 2, "内区顶(+1 框)+表头(+1)+ sel(2)");
        Ok(())
    }

    /// `clamp` 把越界光标钳到 `len-1`,空队列归 0。
    #[test]
    fn clamp_bounds_cursor() {
        let mut o = QueueOverlay::new(9);
        o.clamp(3);
        assert_eq!(o.cursor(), 2);
        o.clamp(0);
        assert_eq!(o.cursor(), 0, "空队列光标归 0");
    }

    /// `on_key` 导航:j/k/g/G 移动光标并 `Consumed`;Enter 产出 `PlayQueueIndex`;
    /// Tab/q/Esc 产出 `CloseTop`;空格 / t 半穿透 `Pass`。
    #[test]
    fn on_key_navigates_and_emits_actions() -> color_eyre::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let ctx = ctx_with_queue(6, None)?;
        let mut o = QueueOverlay::new(0);

        // 导航/激活/关闭走 on_action(语义动作,跟随键位重映射与 behavior 步长)。
        assert!(matches!(
            o.on_action(Action::MoveSelection(SelectionMove::Last), &ctx),
            Some(OverlayResponse::Consumed)
        ));
        assert_eq!(o.cursor(), 5, "Last 跳末行");
        assert!(matches!(
            o.on_action(Action::MoveSelection(SelectionMove::Up(1)), &ctx),
            Some(OverlayResponse::Consumed)
        ));
        assert_eq!(o.cursor(), 4);
        assert!(matches!(
            o.on_action(Action::MoveSelection(SelectionMove::Down(3)), &ctx),
            Some(OverlayResponse::Consumed)
        ));
        assert_eq!(
            o.cursor(),
            5,
            "大步下移越界钳到末行(步长来自注入,不再本地 const)"
        );

        assert!(matches!(
            o.on_action(Action::ActivateSelection, &ctx),
            Some(OverlayResponse::Do(OverlayAction::PlayQueueIndex(5)))
        ));
        // 开关键族(open_queue/quit/back)收敛为关闭本浮层。
        assert!(matches!(
            o.on_action(Action::BackOrClearSearch, &ctx),
            Some(OverlayResponse::Do(OverlayAction::CloseTop))
        ));
        assert!(matches!(
            o.on_action(Action::OpenQueue, &ctx),
            Some(OverlayResponse::Do(OverlayAction::CloseTop))
        ));
        // 播放控制族不认 → None(回落裸键 Pass 半穿透)。
        assert!(o.on_action(Action::TogglePlayPause, &ctx).is_none());
        let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty());
        assert!(matches!(o.on_key(&space, &ctx), OverlayResponse::Pass));
        Ok(())
    }

    /// 造一个歌名可辨的队列(`/` 过滤测试用;名即 id,互异)。
    fn named(names: &[&str]) -> Vec<mineral_model::Song> {
        names.iter().map(|n| mineral_test::song(n)).collect()
    }

    /// `/` 过滤:visible 只留命中行,按匹配分降序(最接近输入在顶);底层队列不动。
    #[test]
    fn filter_keeps_only_matches() -> color_eyre::Result<()> {
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = named(&["alpha", "beta", "gamma", "alba"]);
        let mut o = QueueOverlay::new(0);
        o.search.set_query("al");
        let visible = o.visible(&ctx);
        assert_eq!(visible.len(), 2, "只 alpha/alba 命中");
        assert!(
            visible.contains(&0) && visible.contains(&3),
            "命中行的队列真实下标是 0/3"
        );
        assert!(
            !visible.contains(&1) && !visible.contains(&2),
            "beta/gamma 落选"
        );
        Ok(())
    }

    /// 过滤态 Enter 播的是**队列真实下标**,不是过滤视图位。
    #[test]
    fn activate_plays_raw_index_under_filter() -> color_eyre::Result<()> {
        let mut ctx = AppState::test_default()?;
        // 只有下标 3 命中,过滤后它是视图第 0 行。
        ctx.player.queue = named(&["one", "two", "three", "zephyr"]);
        let mut o = QueueOverlay::new(0);
        o.search.set_query("zephyr");
        assert_eq!(o.raw_cursor(&ctx), Some(3), "视图位 0 映射回真实下标 3");
        assert!(
            matches!(
                o.on_action(Action::ActivateSelection, &ctx),
                Some(OverlayResponse::Do(OverlayAction::PlayQueueIndex(3)))
            ),
            "播真实下标 3"
        );
        Ok(())
    }

    /// 过滤态屏蔽上下移动(视图按分重排后相邻位 ≠ 队列相邻位)。
    #[test]
    fn reorder_suppressed_while_filtering() -> color_eyre::Result<()> {
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = named(&["alpha", "beta", "alba"]);
        let mut o = QueueOverlay::new(0);
        o.search.set_query("al");
        assert!(
            matches!(
                o.on_action(Action::ReorderSelection(SelectionMove::Down(1)), &ctx),
                Some(OverlayResponse::Consumed)
            ),
            "过滤态吞掉 reorder、不发编辑"
        );
        Ok(())
    }

    /// `/` 进输入态 → 逐字改词 → Enter 保留词退输入 → back 清词退出过滤。与浏览页 `/` 同构。
    #[test]
    fn slash_typing_flow_matches_browse() -> color_eyre::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = named(&["alpha", "beta"]);
        let mut o = QueueOverlay::new(0);
        assert!(o.on_action(Action::EnterSearch, &ctx).is_some());
        assert!(o.is_typing(), "`/` 进输入态");
        // 输入态下动作层一律不认(让裸键落 on_key 做文本编辑)。
        assert!(
            o.on_action(Action::MoveSelection(SelectionMove::Down(1)), &ctx)
                .is_none(),
            "输入态 on_action 不认"
        );
        for c in "al".chars() {
            let k = KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty());
            assert!(matches!(o.on_key(&k, &ctx), OverlayResponse::Consumed));
        }
        assert_eq!(o.search.query(), "al");
        o.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()), &ctx);
        assert!(
            !o.is_typing() && o.is_filtering(),
            "Enter 退输入、词保留过滤"
        );
        assert!(
            matches!(
                o.on_action(Action::BackOrClearSearch, &ctx),
                Some(OverlayResponse::Consumed)
            ),
            "过滤态 back 清词(不关浮层)"
        );
        assert!(!o.is_filtering(), "词已清");
        Ok(())
    }

    /// 无过滤时 back 关闭浮层(与既有行为一致,不被 `/` 改道)。
    #[test]
    fn back_closes_when_not_filtering() -> color_eyre::Result<()> {
        let ctx = ctx_with_queue(3, None)?;
        let mut o = QueueOverlay::new(0);
        assert!(matches!(
            o.on_action(Action::BackOrClearSearch, &ctx),
            Some(OverlayResponse::Do(OverlayAction::CloseTop))
        ));
        Ok(())
    }

    /// 拼音首字母过滤:输入 `cry` 命中「春日影」,证明复用了 fuzzy 的拼音段。
    #[test]
    fn pinyin_initials_filter_hits() -> color_eyre::Result<()> {
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = named(&["春日影", "MyGO"]);
        let mut o = QueueOverlay::new(0);
        o.search.set_query("cry");
        assert_eq!(o.visible(&ctx), vec![0], "cry 命中春日影(首字母),MyGO 落选");
        Ok(())
    }

    /// 按**别名**命中:该曲进过滤视图,且命中下标落在别名段(会被高亮),歌名段无命中。
    /// 回归——别名参与打分却漏高亮的 bug。
    #[test]
    fn alias_match_produces_alias_hits() -> color_eyre::Result<()> {
        let mut ctx = AppState::test_default()?;
        // aliased_song:名「迷星叫」/ 别名「Mayoiuta」。"mayo" 只命中别名。
        ctx.player.queue = vec![mineral_test::aliased_song(), mineral_test::song("other")];
        let mut o = QueueOverlay::new(0);
        o.search.set_query("mayo");
        assert_eq!(o.visible(&ctx), vec![0], "按别名命中,该曲进视图");
        let s = ctx
            .player
            .queue
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("队列首项应在"))?;
        let hits = o.row_hits(s);
        assert!(!hits.alias.is_empty(), "别名段命中非空(会被高亮)");
        assert!(hits.name.is_empty(), "命中来自别名,歌名段无命中");
        Ok(())
    }

    /// 跳回在播:在播歌被过滤保留时,光标落到它在**过滤视图**中的位置(非队列真实位)。
    #[test]
    fn jump_to_current_maps_into_filtered_view() -> color_eyre::Result<()> {
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = named(&["alpha", "beta", "gamma", "alba"]);
        // 在播 alba(真实下标 3)。queue_current_index 以 playback.track 为准,须同设。
        ctx.playback.track = ctx.player.queue.get(3).cloned();
        ctx.player.cursor = mineral_protocol::PlayCursor::InQueue(3);
        let mut o = QueueOverlay::new(0);
        o.search.set_query("al");
        let visible = o.visible(&ctx);
        let want = visible
            .iter()
            .position(|&r| r == 3)
            .ok_or_else(|| color_eyre::eyre::eyre!("alba 应在过滤视图内"))?;
        o.on_action(Action::JumpToCurrent, &ctx);
        assert_eq!(o.cursor(), want, "光标落到在播歌在过滤视图中的位置");
        Ok(())
    }

    /// `/al` 过滤态渲染:命中行高亮、按分排序、顶栏 `/al` 输入片段、底栏命中位/命中数。
    #[test]
    fn queue_filtered_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = named(&["alpha", "beta", "gamma", "alba"]);
        let mut overlay = QueueOverlay::new(0);
        overlay.search.set_query("al");
        t.draw(|f| render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "队列浮层:/al 过滤——命中行高亮 + 按分排序 + 顶栏 /al 输入 + 底栏命中位/数",
            t.backend()
        );
        Ok(())
    }

    /// 过滤无命中:画居中占位而非空表(空表会读成「队列为空」)。
    #[test]
    fn queue_no_match_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = named(&["alpha", "beta"]);
        let mut overlay = QueueOverlay::new(0);
        overlay.search.set_query("zzzzz");
        t.draw(|f| render_overlay(f, f.area(), &overlay, 1000, true, &ctx, &Theme::default()))?;
        crate::test_support::assert_snap!("队列浮层:过滤无命中——居中占位", t.backend());
        Ok(())
    }
}
