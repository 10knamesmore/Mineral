//! 浮动 queue 面板:展示当前播放队列,vim 风格导航 + Enter 播放。

use crossterm::event::KeyEvent;
use mineral_model::{Song, SongId};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row, StatefulWidget, Table, TableState};

use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block,
};
use crate::render::theme::Theme;
use crate::runtime::action::{Action, SelectionMove};
use crate::runtime::playback::format_ms;
use crate::runtime::scroll;
use crate::runtime::scroll::ListScroll;
use crate::runtime::state::AppState;

/// 浮动 queue 浮层。
///
/// 只持有 UI-local 光标 `sel`(永不被 server snapshot 覆盖,仅 clamp 防越界);
/// 队列曲目是后端权威态,渲染 / 导航时从 [`AppState`] 读。
pub(crate) struct QueueOverlay {
    /// 光标选中行下标(UI-local)。
    sel: usize,

    /// 队列表格的视口滚动态(nvim 手感 + 缓动平移)。
    scroll: ListScroll,
}

impl QueueOverlay {
    /// 新建:光标定位到 `sel`(打开浮层时通常传在播歌下标)。
    pub(crate) fn new(sel: usize) -> Self {
        let scroll = ListScroll::new();
        // 视口直接落在光标附近(渲染端按 scrolloff 钳),不从队首长程滑过来。
        scroll.snap_to(sel);
        Self { sel, scroll }
    }

    /// 把光标钳到 `[0, len-1]`(队列变短后防越界);空队列归 0。
    pub(crate) fn clamp(&mut self, len: usize) {
        self.sel = self.sel.min(len.saturating_sub(1));
    }

    /// 当前光标行(脚本动作 ctx 采集 / 集成测试断言用)。
    pub(crate) fn cursor(&self) -> usize {
        self.sel
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
                Line::from(position_label(self.sel, ctx.player.queue.len()))
                    .style(Style::new().fg(theme.overlay)),
            )
            .title_bottom(
                Line::from(" ↵ play  ·  Tab/q/esc close ")
                    .right_aligned()
                    .style(Style::new().fg(theme.overlay)),
            )
    }

    fn render_content(&self, buf: &mut Buffer, inner: Rect, ctx: &AppState, theme: &Theme) {
        let current_id = ctx.playback.track.as_ref().map(|t| &t.id);
        // 按浮层内宽选列档:窄浮层放不下 artist 时退到「歌本身」(# title len)。
        let cols = QueueCols::for_width(inner.width);

        let header = Row::new(cols.header_cells())
            .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

        let rows: Vec<Row<'_>> = ctx
            .player
            .queue
            .iter()
            .enumerate()
            .map(|(i, s)| build_row(i, s, current_id, theme, cols))
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
        let offset = self.scroll.render_offset(
            self.sel,
            ctx.player.queue.len(),
            viewport,
            ctx.scrolloff(),
            ctx.list_glide_ticks(),
        );
        let mut state = TableState::default()
            .with_offset(offset)
            .with_selected(Some(scroll::pin_cursor(self.sel, offset, viewport)));
        StatefulWidget::render(table, inner, buf, &mut state);
    }

    fn on_key(&mut self, _key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        // 导航/激活/关闭全走 on_action(跟随键位重映射与 behavior 步长);
        // 未映射裸键半穿透给全局(播放控制族,白名单在 App::passes_overlay)。
        OverlayResponse::Pass
    }

    fn on_action(&mut self, action: Action, ctx: &AppState) -> Option<OverlayResponse> {
        let max = ctx.player.queue.len().saturating_sub(1);
        match action {
            Action::MoveSelection(mv) => {
                self.sel = match mv {
                    SelectionMove::Down(n) => self.sel.saturating_add(n).min(max),
                    SelectionMove::Up(n) => self.sel.saturating_sub(n),
                    SelectionMove::First => 0,
                    SelectionMove::Last => max,
                };
                Some(OverlayResponse::Consumed)
            }
            // `<C-d>` 族:视口目标与光标同移 n 行(vim 语义),边界由渲染端统一钳。
            Action::Scroll(step) => {
                let delta = scroll::step_delta(step, ctx.cfg.tui().behavior());
                let rows = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
                self.scroll.nudge(delta, ctx.list_glide_ticks());
                self.sel = if delta > 0 {
                    self.sel.saturating_add(rows).min(max)
                } else {
                    self.sel.saturating_sub(rows)
                };
                Some(OverlayResponse::Consumed)
            }
            Action::ActivateSelection => {
                Some(OverlayResponse::Do(OverlayAction::PlayQueueIndex(self.sel)))
            }
            // 开关键语义:queue 已开,open_queue(toggle)/ quit / back 都收敛为关闭本浮层。
            Action::OpenQueue | Action::OpenQuitConfirm | Action::BackOrClearSearch => {
                Some(OverlayResponse::Do(OverlayAction::CloseTop))
            }
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
/// 当前在播行:行首 `▶` + 整行 `accent` 前景(与选中行的「背景块」区分);其余行
/// 序号用暗色,歌名用主文本色,艺术家用次要色,层级分明。选中行的高亮背景由
/// Table 的 `row_highlight_style` 叠加,与在播前景着色天然兼容(背景块视觉优先)。
/// `cols` 决定列集:窄档省去 artist。
fn build_row<'a>(
    idx: usize,
    s: &'a Song,
    current_id: Option<&SongId>,
    theme: &Theme,
    cols: QueueCols,
) -> Row<'a> {
    let is_current = current_id.is_some_and(|cid| cid == &s.id);
    let (lead, title_fg, sub_fg) = if is_current {
        (
            Span::styled("▶", Style::new().fg(theme.accent)),
            theme.accent,
            theme.accent,
        )
    } else {
        (
            Span::styled(format!("{idx}"), Style::new().fg(theme.overlay)),
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
        format_ms(s.duration_ms),
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
        s.player.queue = queue;
        Ok(s)
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
        ctx.fullscreen.set(true);
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
