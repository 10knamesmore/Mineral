//! 浮动 queue 面板:展示当前播放队列,vim 风格导航 + Enter 播放。

use crossterm::event::{KeyCode, KeyEvent};
use mineral_model::{Song, SongId};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row, Table, TableState};

use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block,
};
use crate::render::theme::Theme;
use crate::runtime::playback::format_ms;
use crate::runtime::state::AppState;

/// queue 大跳行数(`J` / `K` 一次)。j/k/箭头仍是 1。
const ROW_BIG_STEP: usize = 7;

/// 浮动 queue 浮层。
///
/// 只持有 UI-local 光标 `sel`(永不被 server snapshot 覆盖,仅 clamp 防越界);
/// 队列曲目是后端权威态,渲染 / 导航时从 [`AppState`] 读。
pub(crate) struct QueueOverlay {
    /// 光标选中行下标(UI-local)。
    sel: usize,
}

impl QueueOverlay {
    /// 新建:光标定位到 `sel`(打开浮层时通常传在播歌下标)。
    pub(crate) fn new(sel: usize) -> Self {
        Self { sel }
    }

    /// 把光标钳到 `[0, len-1]`(队列变短后防越界);空队列归 0。
    pub(crate) fn clamp(&mut self, len: usize) {
        self.sel = self.sel.min(len.saturating_sub(1));
    }

    /// 当前光标行(供栈聚合给集成测试断言)。
    #[cfg(test)]
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
                Line::from(position_label(self.sel, ctx.queue.len()))
                    .style(Style::new().fg(theme.overlay)),
            )
            .title_bottom(
                Line::from(" ↵ play  ·  Tab/q/esc close ")
                    .right_aligned()
                    .style(Style::new().fg(theme.overlay)),
            )
    }

    fn render_content(&self, frame: &mut Frame<'_>, inner: Rect, ctx: &AppState, theme: &Theme) {
        let current_id = ctx.playback.track.as_ref().map(|t| &t.id);
        // 按浮层内宽选列档:窄浮层放不下 artist 时退到「歌本身」(# title len)。
        let cols = QueueCols::for_width(inner.width);

        let header = Row::new(cols.header_cells())
            .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

        let rows: Vec<Row<'_>> = ctx
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

        let mut state = TableState::default();
        state.select(Some(self.sel));
        frame.render_stateful_widget(table, inner, &mut state);
    }

    fn on_key(&mut self, key: &KeyEvent, ctx: &AppState) -> OverlayResponse {
        let len = ctx.queue.len();
        let max = len.saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.sel = self.sel.saturating_add(1).min(max);
                OverlayResponse::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sel = self.sel.saturating_sub(1);
                OverlayResponse::Consumed
            }
            KeyCode::Char('J') => {
                self.sel = self.sel.saturating_add(ROW_BIG_STEP).min(max);
                OverlayResponse::Consumed
            }
            KeyCode::Char('K') => {
                self.sel = self.sel.saturating_sub(ROW_BIG_STEP);
                OverlayResponse::Consumed
            }
            KeyCode::Char('g') => {
                self.sel = 0;
                OverlayResponse::Consumed
            }
            KeyCode::Char('G') => {
                self.sel = max;
                OverlayResponse::Consumed
            }
            KeyCode::Enter => OverlayResponse::Do(OverlayAction::PlayQueueIndex(self.sel)),
            KeyCode::Tab | KeyCode::Char('q') | KeyCode::Esc => {
                OverlayResponse::Do(OverlayAction::CloseTop)
            }
            // 其余(空格 / m / ±/ ←→ / p / n / t)半穿透给全局键:queue 打开时仍可控播放、切歌词。
            _ => OverlayResponse::Pass,
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
    use crate::runtime::state::AppState;
    use crate::test_support::endserenading;

    /// 造一个填了队列的 [`AppState`],并把在播歌设为下标 `current`(None 表示无在播)。
    fn ctx_with_queue(len: usize, current: Option<usize>) -> AppState {
        let mut s = AppState::empty();
        let queue = endserenading(len);
        s.playback.track = current.and_then(|i| queue.get(i).cloned());
        s.queue = queue;
        s
    }

    /// 空 queue,完全展开。
    #[test]
    fn queue_empty_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let ctx = AppState::empty();
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
        let ctx = ctx_with_queue(3, Some(1));
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

    /// 小终端(backend=60)浮层被钳到 min_w=40 → inner 38 < 44 → Song 档:
    /// 只剩 # / title / len,artist 省去。
    #[test]
    fn queue_narrow_song_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let ctx = ctx_with_queue(3, Some(1));
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

    /// 弹出动画半程(scale=500):只画缩放后的空壳边框,无表格内容。
    #[test]
    fn queue_mid_animation_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let ctx = ctx_with_queue(3, None);
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
        crate::test_support::assert_snap!("队列浮层:弹出动画半程(scale=500)空壳", t.backend());
        Ok(())
    }

    /// 弹出动画 scale=510:宽高都落在非整 cell,四条边各有 1/8 块过渡(上/下沿下八分块、
    /// 左/右沿左八分块、角点垂直近似),验证双轴平滑。
    #[test]
    fn queue_smooth_both_axes_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        let ctx = ctx_with_queue(3, None);
        let overlay = QueueOverlay::new(0);
        t.draw(|f| {
            render_overlay(
                f,
                f.area(),
                &overlay,
                /*scale*/ 510,
                true,
                &ctx,
                &Theme::default(),
            );
        })?;
        crate::test_support::assert_snap!("队列浮层:弹出动画 scale=510 双轴 1/8 平滑", t.backend());
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
    fn on_key_navigates_and_emits_actions() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let ctx = ctx_with_queue(6, None);
        let key = |c: KeyCode| KeyEvent::new(c, KeyModifiers::empty());
        let mut o = QueueOverlay::new(0);

        assert!(matches!(
            o.on_key(&key(KeyCode::Char('G')), &ctx),
            OverlayResponse::Consumed
        ));
        assert_eq!(o.cursor(), 5, "G 跳末行");
        assert!(matches!(
            o.on_key(&key(KeyCode::Char('k')), &ctx),
            OverlayResponse::Consumed
        ));
        assert_eq!(o.cursor(), 4);

        assert!(matches!(
            o.on_key(&key(KeyCode::Enter), &ctx),
            OverlayResponse::Do(OverlayAction::PlayQueueIndex(4))
        ));
        assert!(matches!(
            o.on_key(&key(KeyCode::Esc), &ctx),
            OverlayResponse::Do(OverlayAction::CloseTop)
        ));
        assert!(matches!(
            o.on_key(&key(KeyCode::Char(' ')), &ctx),
            OverlayResponse::Pass
        ));
    }
}
