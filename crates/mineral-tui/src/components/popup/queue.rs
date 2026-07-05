//! 浮动 queue 面板:展示当前播放队列,vim 风格导航 + Enter 播放。

use crossterm::event::KeyEvent;
use mineral_model::Song;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row, Table};

use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block, dock_full_rect,
};
use crate::render::theme::{Theme, resolve_source_color};
use crate::runtime::action::Action;
use crate::runtime::playback::format_ms_opt;
use crate::runtime::scroll;
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::state::AppState;

/// 浮动 queue 浮层。
///
/// 只持有 UI-local 光标 + 视口滚动(`list`,永不被 server snapshot 覆盖,仅 clamp 防越界);
/// 队列曲目是后端权威态,渲染 / 导航时从 [`AppState`] 读。
pub(crate) struct QueueOverlay {
    /// 光标 + 视口滚动态(UI-local;走通用 [`ScrollList`])。
    list: ScrollList,
}

impl QueueOverlay {
    /// 新建:光标 + 视口直接定位到 `sel`(打开浮层时通常传在播歌下标),不从队首长程滑过来。
    pub(crate) fn new(sel: usize) -> Self {
        Self {
            list: ScrollList::at(sel),
        }
    }

    /// 把光标钳到 `[0, len-1]`(队列变短后防越界);空队列归 0。
    pub(crate) fn clamp(&mut self, len: usize) {
        self.list.clamp(len);
    }

    /// 当前光标行(脚本动作 ctx 采集 / 集成测试断言用)。
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
        base_block(theme)
            .border_style(Style::new().fg(border_color))
            .title(Line::from(" queue ").style(Style::new().fg(theme.subtext)))
            .title_bottom(
                Line::from(position_label(self.list.sel(), ctx.player.queue.len()))
                    .style(Style::new().fg(theme.overlay)),
            )
            .title_bottom(
                Line::from(" ↵ play  ·  Tab/q/esc close ")
                    .right_aligned()
                    .style(Style::new().fg(theme.overlay)),
            )
    }

    fn render_content(&self, buf: &mut Buffer, inner: Rect, ctx: &AppState, theme: &Theme) {
        // ▶ 标记按 server 的在播位置锚点定位(queue_current_index,下标优先),
        // 不用歌曲身份匹配——队列含重复曲时身份会把所有副本一起点亮。
        let current_idx = ctx.queue_current_index();
        // 按浮层内宽选列档:窄浮层放不下 artist 时退到「歌本身」(# title len)。
        let cols = QueueCols::for_width(inner.width);
        // 序号无条件染该行歌曲的源色(零列宽成本地表示来源):同源队列整列同色即
        // 该队列来源,混源队列则逐行不同。
        let header = Row::new(cols.header_cells())
            .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

        let rows: Vec<Row<'_>> = ctx
            .player
            .queue
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let index_fg = resolve_source_color(theme, ctx.cfg.sources(), s.source());
                build_row(i, s, current_idx, theme, cols, index_fg)
            })
            .collect();

        let widths = cols.widths();

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(
                Style::new()
                    .bg(theme.surface0)
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▌ ");

        // 视口行数 = 内区高 - 表头(边框归浮层 chrome);offset 跨帧持久 + 缓动平移。
        let viewport = usize::from(inner.height.saturating_sub(1));
        render_scroll_table(
            buf,
            inner,
            table,
            &self.list,
            ctx.player.queue.len(),
            viewport,
            ScrollMotion::Advancing {
                scrolloff: ctx.scrolloff(),
                glide_ticks: ctx.list_glide_ticks(),
            },
        );
    }

    fn on_key(&mut self, _key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        // 导航/激活/关闭全走 on_action(跟随键位重映射与 behavior 步长);
        // 未映射裸键半穿透给全局(播放控制族,白名单在 App::passes_overlay)。
        OverlayResponse::Pass
    }

    fn on_action(&mut self, action: Action, ctx: &AppState) -> Option<OverlayResponse> {
        let len = ctx.player.queue.len();
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
            Action::ActivateSelection => Some(OverlayResponse::Do(OverlayAction::PlayQueueIndex(
                self.list.sel(),
            ))),
            // 开关键语义:queue 已开,open_queue(toggle)/ quit / back 都收敛为关闭本浮层。
            Action::OpenQueue | Action::OpenQuitConfirm | Action::BackOrClearSearch => {
                Some(OverlayResponse::Do(OverlayAction::CloseTop))
            }
            // `y`:为当前光标行弹复制菜单(贴行下方、叠在 queue 之上);queue 只读复制,不改队列。
            Action::OpenCopyMenu => Some(OverlayResponse::Do(OverlayAction::CopyQueueIndex {
                idx: self.list.sel(),
                anchor: self.row_anchor(ctx),
            })),
            // `o`:queue 内无「容器/单曲操作」语义(它本身即队列),显式吞掉(Consumed)而非
            // 回落——意图直白,且不押注 passes_overlay 白名单恰好不含 OpenActionMenu 的现状。
            Action::OpenActionMenu => Some(OverlayResponse::Consumed),
            // 其余(播放控制族等)不认 → 回落 on_key(Pass 半穿透)。
            _ => None,
        }
    }
}

/// queue 表格的列档,按浮层内宽选(见 [`QueueCols::for_width`])。
#[derive(Clone, Copy)]
enum QueueCols {
    /// 宽档:# / title / artist / len,文本列比例 Fill(3:2)。
    Full,

    /// 窄档:# / title / len —— artist 放不下,退到「歌本身」。
    Song,
}

impl QueueCols {
    /// 按浮层内宽 `width` 选档。阈值 44:低于此 title/artist 各分不到约 14 格,
    /// 退到只剩歌名。
    fn for_width(width: u16) -> Self {
        if width < 44 { Self::Song } else { Self::Full }
    }

    /// 表头单元格(与 [`Self::widths`] / [`build_row`] 的列集严格一致)。
    fn header_cells(self) -> Vec<Cell<'static>> {
        let mut cells = vec![Cell::from("#"), Cell::from("title")];
        if matches!(self, Self::Full) {
            cells.push(Cell::from("artist"));
        }
        cells.push(Cell::from("len"));
        cells
    }

    /// 列宽约束:`#` / len 定宽,文本列比例 Fill。
    fn widths(self) -> Vec<Constraint> {
        match self {
            Self::Full => vec![
                Constraint::Length(3),
                Constraint::Fill(3),
                Constraint::Fill(2),
                Constraint::Length(6),
            ],
            Self::Song => vec![
                Constraint::Length(3),
                Constraint::Fill(1),
                Constraint::Length(6),
            ],
        }
    }
}

/// 把一首歌组成 queue 表格的一行。
///
/// 当前在播行:行首 `▶` + 整行 `accent` 前景(与选中行的「背景块」区分),在播标记
/// 语义优先于源色;其余行序号用 `index_fg`(该行歌曲的源色,整列同色即队列来源),
/// 歌名用主文本色,艺术家用次要色,层级分明。选中行的高亮背景由 Table 的
/// `row_highlight_style` 叠加,与在播前景着色天然兼容(背景块视觉优先)。
/// `cols` 决定列集:窄档省去 artist。
fn build_row<'a>(
    idx: usize,
    s: &'a Song,
    current_idx: Option<usize>,
    theme: &Theme,
    cols: QueueCols,
    index_fg: ratatui::style::Color,
) -> Row<'a> {
    let is_current = current_idx == Some(idx);
    let (lead, title_fg, sub_fg) = if is_current {
        (
            Span::styled("▶", Style::new().fg(theme.accent)),
            theme.accent,
            theme.accent,
        )
    } else {
        (
            Span::styled(format!("{idx}"), Style::new().fg(index_fg)),
            theme.text,
            theme.subtext,
        )
    };

    let mut cells = vec![
        Cell::from(lead),
        Cell::from(Span::styled(s.name.clone(), Style::new().fg(title_fg))),
    ];
    if matches!(cols, QueueCols::Full) {
        let artist = s
            .artists
            .first()
            .map_or_else(|| "—".to_owned(), |a| a.name.clone());
        cells.push(Cell::from(Span::styled(artist, Style::new().fg(sub_fg))));
    }
    cells.push(Cell::from(Span::styled(
        format_ms_opt(s.duration_ms),
        Style::new().fg(sub_fg),
    )));
    Row::new(cells)
}

/// 拼 ` n / total ` 的 footer 标签;空 queue 显示 `0 / 0`。
fn position_label(sel: usize, total: usize) -> String {
    if total == 0 {
        " 0 / 0 ".to_owned()
    } else {
        format!(" {} / {total} ", sel.saturating_add(1).min(total))
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
        // queue_sel 是 ▶ 标记的下标依据(server 在播锚点的镜像)。
        s.player.queue_sel = current.unwrap_or(0);
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

    /// 回归:队列含重复曲、在播锚点(queue_sel)落在**第二个**副本时,
    /// 只有那一行标 `▶`——历史 bug 按歌曲身份匹配会把两个副本一起点亮(count==2)。
    #[test]
    fn queue_duplicate_marks_only_anchor_row() -> color_eyre::Result<()> {
        use mineral_test::song;
        let mut t = Terminal::new(TestBackend::new(100, 24))?;
        let mut ctx = AppState::test_default()?;
        ctx.player.queue = vec![song("a"), song("b"), song("a"), song("b")];
        ctx.player.queue_sel = 2; // 第二个 a 正在播
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
    /// backend=100 → 浮层够宽落 Full 档(# / title / artist / len)。
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

    /// 小终端(backend=60)停靠浮层宽 = 左 64% ≈ 38 → inner < 44 → Song 档:
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
        assert!(
            matches!(
                o.on_action(Action::OpenActionMenu, &ctx),
                Some(OverlayResponse::Consumed)
            ),
            "o 在 queue 被显式吞掉(无操作语义)"
        );
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
}
