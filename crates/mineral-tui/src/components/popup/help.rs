//! 键位 cheatsheet 浮层:全量键位按功能分组,多列瀑布一屏可览。
//!
//! 目录来自 keymap 的 help 快照(用户重映射 / 脚本绑定自动跟随),行形是
//! 「label 左对齐 · 点线牵引 · chip 键帽收右缘」;列数由内容最小宽自适应,
//! 放不下时整张表随滚动键上下平移(右缘滚动条指示)。

use std::cell::Cell;

use crossterm::event::KeyEvent;
use mineral_config::keys::KeyChord;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use unicode_width::UnicodeWidthStr;

use crate::components::layout::shared::scrollbar::draw_scrollbar;
use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block,
};
use crate::render::theme::Theme;
use crate::runtime::action::{Action, SelectionMove};
use crate::runtime::keymap::help::HelpEntry;
use crate::runtime::scroll;
use crate::runtime::state::AppState;

/// 每行最多露出的键帽数;同义余键折进 `+N`。
const MAX_CHIPS: usize = 2;

/// 点线牵引段的最小格数(短于此宁可截断 label)。
const DOTS_MIN: usize = 2;

/// 列间距(格)。
const GUTTER: usize = 2;

/// 最多列数。
const MAX_COLS: usize = 3;

/// 键位 cheatsheet 浮层。目录与关闭提示在打开瞬间从 keymap 快照,浮层自身
/// 只持有滚动态(溢出上界由渲染端按实际布局回填,动作端 clamp 用)。
pub(crate) struct HelpOverlay {
    /// 目录快照(显示顺序,同组条目连续)。
    entries: Vec<HelpEntry>,

    /// 底边关闭提示的键文本(chip 短形);`open_help` 被解绑时无提示。
    close_hint: Option<String>,

    /// 视口首行(整张表统一平移;不溢出时恒 0)。
    scroll: Cell<usize>,

    /// 滚动上界 = 表高 − 视口高(渲染端按实际布局回填;`Last` / clamp 用)。
    max_scroll: Cell<usize>,
}

impl HelpOverlay {
    /// 新建 cheatsheet 浮层。
    ///
    /// # Params:
    ///   - `entries`: keymap help 目录快照
    ///   - `close_hint`: 关闭键的 chip 文本(`open_help` 反查;解绑为 `None`)
    pub(crate) fn new(entries: Vec<HelpEntry>, close_hint: Option<String>) -> Self {
        Self {
            entries,
            close_hint,
            scroll: Cell::new(0),
            max_scroll: Cell::new(0),
        }
    }

    /// 视口平移 `delta` 行,钳在 `[0, max_scroll]`(上界来自最近一帧渲染)。
    fn scroll_by(&self, delta: i64) {
        let cur = i64::try_from(self.scroll.get()).unwrap_or(i64::MAX);
        let max = self.max_scroll.get();
        let next = usize::try_from(cur.saturating_add(delta).max(0)).unwrap_or(0);
        self.scroll.set(next.min(max));
    }
}

impl Overlay for HelpOverlay {
    fn chrome(&self) -> Chrome {
        Chrome {
            pct_w: 80,
            pct_h: 80,
            min_w: 40,
            min_h: 12,
            max_w: 130,
            // 默认目录 3 列约 15 行、含脚本组也在 18 内;再高只剩空白。
            max_h: 20,
            animated: true,
            dock: false,
            anchor: None,
            align: None,
        }
    }

    fn block(&self, _ctx: &AppState, theme: &Theme, focused: bool) -> Block<'static> {
        let border_color = if focused {
            theme.accent
        } else {
            theme.surface1
        };
        let block = base_block(theme)
            .border_style(Style::new().fg(border_color))
            .title(Line::from(" Key Cheatsheet ").style(Style::new().fg(theme.subtext)));
        match &self.close_hint {
            Some(hint) => block.title_bottom(
                Line::from(format!(" {hint} close "))
                    .right_aligned()
                    .style(Style::new().fg(theme.overlay)),
            ),
            None => block,
        }
    }

    fn render_content(&self, buf: &mut Buffer, inner: Rect, _ctx: &AppState, theme: &Theme) {
        // 左缘留 1 格边距;右缘固定留 1 格滚动条道(不溢出时是空白边距,
        // 避免溢出瞬间列宽跳变)。
        let content_w = usize::from(inner.width.saturating_sub(2));
        let viewport = usize::from(inner.height);
        if content_w < 8 || viewport == 0 || self.entries.is_empty() {
            return;
        }
        let rows = self
            .entries
            .iter()
            .map(|e| EntryRow::new(e, theme))
            .collect::<Vec<EntryRow>>();
        let blocks = group_blocks(&self.entries);
        // 列数由内容最小宽推导(最宽行 + 点线最小段),钳 [1, MAX_COLS];列宽均分。
        let min_col_w = rows
            .iter()
            .map(EntryRow::min_width)
            .chain(blocks.iter().map(|b| b.title.width().saturating_add(4)))
            .max()
            .unwrap_or(content_w);
        let cols = ((content_w.saturating_add(GUTTER)) / (min_col_w.saturating_add(GUTTER)))
            .clamp(1, MAX_COLS)
            .min(blocks.len().max(1));
        let col_w = content_w
            .saturating_sub(GUTTER.saturating_mul(cols.saturating_sub(1)))
            .checked_div(cols)
            .unwrap_or(content_w);
        // 组为单位顺序分列,最小化最大列高(组内聚,不跨列拆)。
        let heights = blocks
            .iter()
            .map(|b| b.rows.len().saturating_add(1))
            .collect::<Vec<usize>>();
        let parts = partition_blocks(&heights, cols);
        let columns = parts
            .iter()
            .map(|range| {
                let mut lines = Vec::<Line<'static>>::new();
                for bi in range.clone() {
                    let Some(block) = blocks.get(bi) else {
                        continue;
                    };
                    if !lines.is_empty() {
                        lines.push(Line::default());
                    }
                    lines.push(block.header_line(col_w, theme));
                    for ri in block.rows.clone() {
                        if let Some(row) = rows.get(ri) {
                            lines.push(row.line(col_w, theme));
                        }
                    }
                }
                lines
            })
            .collect::<Vec<Vec<Line<'static>>>>();
        // 溢出滚动:整张表统一平移,上界回填给动作端。
        let sheet_h = columns.iter().map(Vec::len).max().unwrap_or(0);
        let max_scroll = sheet_h.saturating_sub(viewport);
        self.max_scroll.set(max_scroll);
        let offset = self.scroll.get().min(max_scroll);
        self.scroll.set(offset);

        for (ci, column) in columns.iter().enumerate() {
            let x_off = ci
                .saturating_mul(col_w.saturating_add(GUTTER))
                .saturating_add(1);
            let x = inner
                .x
                .saturating_add(u16::try_from(x_off).unwrap_or(u16::MAX));
            for (ri, line) in column.iter().skip(offset).take(viewport).enumerate() {
                let y = inner
                    .y
                    .saturating_add(u16::try_from(ri).unwrap_or(u16::MAX));
                buf.set_line(x, y, line, u16::try_from(col_w).unwrap_or(inner.width));
            }
        }
        if max_scroll > 0 {
            // 定长滑块滚动条:ratatui 内置 Scrollbar 滑块长随 position 波动会蠕动。
            draw_scrollbar(
                buf,
                inner,
                u16::try_from(sheet_h).unwrap_or(u16::MAX),
                u16::try_from(viewport).unwrap_or(u16::MAX),
                u16::try_from(offset).unwrap_or(u16::MAX),
                theme,
            );
        }
    }

    fn on_key(&mut self, _key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        // 未映射裸键半穿透给全局(播放控制族白名单在 App::passes_overlay)。
        OverlayResponse::Pass
    }

    fn on_action(&mut self, action: Action, ctx: &AppState) -> Option<OverlayResponse> {
        match action {
            // 开关键语义:help 已开,open_help(toggle)/ back / quit 都收敛为关闭。
            Action::OpenHelp | Action::BackOrClearSearch | Action::OpenQuitConfirm => {
                Some(OverlayResponse::Do(OverlayAction::CloseTop))
            }
            Action::MoveSelection(mv) => {
                match mv {
                    SelectionMove::Down(n) => {
                        self.scroll_by(i64::try_from(n).unwrap_or(i64::MAX));
                    }
                    SelectionMove::Up(n) => {
                        self.scroll_by(i64::try_from(n).unwrap_or(i64::MAX).saturating_neg());
                    }
                    SelectionMove::First => self.scroll.set(0),
                    SelectionMove::Last => self.scroll.set(self.max_scroll.get()),
                }
                Some(OverlayResponse::Consumed)
            }
            Action::Scroll(step) => {
                self.scroll_by(scroll::viewport::step_delta(step, ctx.cfg.tui().behavior()));
                Some(OverlayResponse::Consumed)
            }
            // 播放控制族 + 歌词切换 + 关通知:不认 → 回落裸键 Pass(半穿透,边看边试)。
            Action::TogglePlayPause
            | Action::CyclePlayMode
            | Action::NudgeVolume(_)
            | Action::SeekRelative(_)
            | Action::PrevOrRestart
            | Action::NextSong
            | Action::CycleLyricExtra
            | Action::DismissNotice => None,
            // 其余(列表激活 / 下载 / 菜单 / 布局切换…)显式吞掉:cheatsheet 盖住
            // 主视图,不能让动作打在看不见的列表上。
            Action::ToggleFullscreen
            | Action::OpenSearchView
            | Action::OpenQueue
            | Action::EnterSearch
            | Action::ActivateSelection
            | Action::DrillIntoSelection
            | Action::CycleDetailSection
            | Action::ToggleLoveSelection
            | Action::DownloadSelection
            | Action::OpenActionMenu
            | Action::OpenCopyMenu
            | Action::InvokeScript(_) => Some(OverlayResponse::Consumed),
        }
    }
}

/// 一个分组在渲染前的形态:标题 + 行下标区间(指向条目预渲染表)。
struct GroupBlock {
    /// 组标题(`HelpGroup::title`)。
    title: &'static str,

    /// 组内条目在预渲染表里的下标区间。
    rows: std::ops::Range<usize>,
}

impl GroupBlock {
    /// 组标题行:标题(accent_2 加粗)+ 细线拖满列宽。
    fn header_line(&self, col_w: usize, theme: &Theme) -> Line<'static> {
        let rule_w = col_w.saturating_sub(self.title.width().saturating_add(1));
        Line::from(vec![
            Span::styled(
                self.title.to_owned(),
                Style::new().fg(theme.accent_2).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled("─".repeat(rule_w), Style::new().fg(theme.surface1)),
        ])
    }
}

/// 把目录按组切块(条目已保证同组连续)。
fn group_blocks(entries: &[HelpEntry]) -> Vec<GroupBlock> {
    let mut blocks = Vec::<GroupBlock>::new();
    for (i, entry) in entries.iter().enumerate() {
        match blocks.last_mut() {
            Some(last) if last.title == entry.group().title() => {
                last.rows.end = i.saturating_add(1);
            }
            _ => blocks.push(GroupBlock {
                title: entry.group().title(),
                rows: i..i.saturating_add(1),
            }),
        }
    }
    blocks
}

/// 一条键位行的预渲染形态:label + 键帽 span 序列(测宽复用)。
struct EntryRow {
    /// 英文短描述。
    label: String,

    /// 键帽区 span(chip / 帽间空格 / `+N` 折叠标记)。
    keys: Vec<Span<'static>>,

    /// 键帽区总显示宽。
    keys_w: usize,
}

impl EntryRow {
    /// 预渲染一条目录行:前 [`MAX_CHIPS`] 个键做键帽,余键折 `+N`。
    fn new(entry: &HelpEntry, theme: &Theme) -> Self {
        let chip_style = Style::new().bg(theme.surface0).fg(theme.peach);
        let mut keys = Vec::<Span<'static>>::new();
        for chord in entry.chords().iter().take(MAX_CHIPS) {
            if !keys.is_empty() {
                keys.push(Span::raw(" "));
            }
            keys.push(Span::styled(format!(" {} ", chip_text(*chord)), chip_style));
        }
        let folded = entry.chords().len().saturating_sub(MAX_CHIPS);
        if folded > 0 {
            keys.push(Span::styled(
                format!(" +{folded}"),
                Style::new().fg(theme.overlay),
            ));
        }
        let keys_w = keys.iter().map(Span::width).sum::<usize>();
        Self {
            label: entry.label().to_owned(),
            keys,
            keys_w,
        }
    }

    /// 本行不截断 label 所需的最小列宽(label + 空格 + 最小点线 + 空格 + 键帽区)。
    fn min_width(&self) -> usize {
        self.label
            .width()
            .saturating_add(DOTS_MIN)
            .saturating_add(2)
            .saturating_add(self.keys_w)
    }

    /// 组装成一行:label 左对齐、点线补中、键帽收右缘;列宽塞不下时截断 label。
    fn line(&self, col_w: usize, theme: &Theme) -> Line<'static> {
        let label_budget = col_w
            .saturating_sub(self.keys_w)
            .saturating_sub(DOTS_MIN.saturating_add(2));
        let label = truncate_to_width(&self.label, label_budget);
        let dots_w = col_w
            .saturating_sub(label.width())
            .saturating_sub(self.keys_w)
            .saturating_sub(2);
        let mut spans = vec![
            Span::styled(label, Style::new().fg(theme.text)),
            Span::raw(" "),
            Span::styled("·".repeat(dots_w), Style::new().fg(theme.surface1)),
            Span::raw(" "),
        ];
        spans.extend(self.keys.iter().cloned());
        Line::from(spans)
    }
}

/// 顺序分列(线性划分):把连续组块切成 ≤ `cols` 段,最小化最大段高
/// (段高 = 组高之和 + 段内组间各 1 空行)。组内聚是硬约束,故是经典
/// linear partition,块数少(≤ 组数)直接 DP。
///
/// # Params:
///   - `heights`: 各组块高(标题 + 行数)
///   - `cols`: 目标段数(≥ 1)
///
/// # Return:
///   每段的块下标区间,依序覆盖全部块(段数 ≤ `cols`)
fn partition_blocks(heights: &[usize], cols: usize) -> Vec<std::ops::Range<usize>> {
    let n = heights.len();
    if n == 0 || cols <= 1 {
        return std::iter::once(0..n).collect::<Vec<std::ops::Range<usize>>>();
    }
    let cols = cols.min(n);
    // seg(i, j) = heights[i..j] 之和 + (j-i-1) 空行。
    let mut prefix = Vec::<usize>::with_capacity(n.saturating_add(1));
    prefix.push(0);
    for h in heights {
        let last = prefix.last().copied().unwrap_or(0);
        prefix.push(last.saturating_add(*h));
    }
    let seg = |i: usize, j: usize| -> usize {
        let sum = prefix
            .get(j)
            .copied()
            .unwrap_or(0)
            .saturating_sub(prefix.get(i).copied().unwrap_or(0));
        sum.saturating_add(j.saturating_sub(i).saturating_sub(1))
    };
    // dp[k][j] = 前 j 块切成 k 段的最小最大段高;break_at 记回溯点。
    let mut dp = vec![vec![usize::MAX; n.saturating_add(1)]; cols.saturating_add(1)];
    let mut break_at = vec![vec![0usize; n.saturating_add(1)]; cols.saturating_add(1)];
    if let Some(row) = dp.get_mut(1) {
        for j in 1..=n {
            if let Some(cell) = row.get_mut(j) {
                *cell = seg(0, j);
            }
        }
    }
    for k in 2..=cols {
        for j in k..=n {
            for i in (k.saturating_sub(1))..j {
                let prev = dp
                    .get(k.saturating_sub(1))
                    .and_then(|r| r.get(i))
                    .copied()
                    .unwrap_or(usize::MAX);
                let cand = prev.max(seg(i, j));
                let cur = dp
                    .get(k)
                    .and_then(|r| r.get(j))
                    .copied()
                    .unwrap_or(usize::MAX);
                if cand < cur {
                    if let Some(cell) = dp.get_mut(k).and_then(|r| r.get_mut(j)) {
                        *cell = cand;
                    }
                    if let Some(cell) = break_at.get_mut(k).and_then(|r| r.get_mut(j)) {
                        *cell = i;
                    }
                }
            }
        }
    }
    // 选段数:更多段只在真降低最大高时才用(避免为凑列数切出碎段)。
    let mut best_k = 1;
    let mut best = dp
        .get(1)
        .and_then(|r| r.get(n))
        .copied()
        .unwrap_or(usize::MAX);
    for k in 2..=cols {
        let v = dp
            .get(k)
            .and_then(|r| r.get(n))
            .copied()
            .unwrap_or(usize::MAX);
        if v < best {
            best = v;
            best_k = k;
        }
    }
    let mut bounds = vec![n];
    let mut j = n;
    let mut k = best_k;
    while k > 1 {
        j = break_at.get(k).and_then(|r| r.get(j)).copied().unwrap_or(0);
        bounds.push(j);
        k = k.saturating_sub(1);
    }
    bounds.push(0);
    bounds.reverse();
    bounds
        .windows(2)
        .filter_map(|w| match (w.first(), w.get(1)) {
            (Some(&a), Some(&b)) => Some(a..b),
            _ => None,
        })
        .collect::<Vec<std::ops::Range<usize>>>()
}

/// 按显示宽截断字符串,截断时末位补 `…`;宽度充足原样返回。
fn truncate_to_width(s: &str, max_w: usize) -> String {
    if s.width() <= max_w {
        return s.to_owned();
    }
    let mut out = String::new();
    let budget = max_w.saturating_sub(1);
    for c in s.chars() {
        let mut probe = out.clone();
        probe.push(c);
        if probe.width() > budget {
            break;
        }
        out = probe;
    }
    out.push('…');
    out
}

/// chip 键帽内的短文本:把和弦的规范 nvim 记法压缩——裸字符原样、尖括号剥壳、
/// 方向键换箭头字形、修饰前缀(`C-` / `S-`)保留、具名键(Space/CR/Esc/BS/Tab)
/// 保留单词。
///
/// # Params:
///   - `chord`: 归一化和弦
///
/// # Return:
///   键帽文本(不含两侧留白)
pub(crate) fn chip_text(chord: KeyChord) -> String {
    let canonical = chord.to_string();
    // 裸字符键(无修饰、非空格)无尖括号:原样即键帽文本。
    let Some(inner) = canonical
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
    else {
        return canonical;
    };
    // 末段是键名、前缀是修饰段(含尾随 `-`);`--` 结尾 = 键名本身是 `-`
    // (与 `KeyChord::parse` 的切分规则镜像)。
    let (mods, name) = if inner.ends_with("--") {
        inner.split_at(inner.len().saturating_sub(1))
    } else {
        match inner.rfind('-') {
            Some(idx) => inner.split_at(idx.saturating_add(1)),
            None => ("", inner),
        }
    };
    let name = match name {
        "Left" => "←",
        "Right" => "→",
        "Up" => "↑",
        "Down" => "↓",
        other => other,
    };
    format!("{mods}{name}")
}

#[cfg(test)]
mod tests {
    use mineral_config::keys::KeyChord;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::{HelpOverlay, chip_text};
    use crate::components::popup::component::render_overlay;
    use crate::render::theme::Theme;
    use crate::runtime::action::Action;
    use crate::runtime::keymap::Keymap;
    use crate::runtime::state::AppState;

    /// 用 defaults 配置的 keymap 目录造一个 cheatsheet 浮层(生产同路径)。
    fn help_overlay_from_defaults() -> color_eyre::Result<HelpOverlay> {
        let cfg = mineral_config::Config::defaults()?;
        let km = Keymap::from_config(cfg.tui().keys(), cfg.tui().behavior());
        Ok(HelpOverlay::new(
            km.help().to_vec(),
            km.hint_chord(Action::OpenHelp).map(chip_text),
        ))
    }

    /// 宽终端(130×32):多列瀑布、组标题 + 细线、label 左 · 点线 · chip 键帽右、
    /// 多键条目只露前 2 帽 + `+N`,一屏放完无滚动指示。
    #[test]
    fn help_wide_columns_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(130, 32))?;
        let ctx = AppState::test_default()?;
        let overlay = help_overlay_from_defaults()?;
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
        crate::test_support::assert_snap!(
            "cheatsheet 浮层:宽终端多列瀑布(label 左 · 点线 · chip 键帽右)",
            t.backend()
        );
        Ok(())
    }

    /// 窄矮终端(46×18):塌到单列,放不完 → 底边出现行窗指示,内容从首组开始。
    #[test]
    fn help_narrow_single_column_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(46, 18))?;
        let ctx = AppState::test_default()?;
        let overlay = help_overlay_from_defaults()?;
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
        crate::test_support::assert_snap!(
            "cheatsheet 浮层:窄矮终端单列 + 溢出滚动指示",
            t.backend()
        );
        Ok(())
    }

    /// chip 键帽文本:nvim 记法压缩——裸字符原样、尖括号剥壳、方向键换箭头字形、
    /// 修饰前缀保留、具名键(Space/CR/Esc/BS/Tab)保留单词。
    #[test]
    fn chip_text_compacts_nvim_notation() -> color_eyre::Result<()> {
        let cases = [
            ("j", "j"),
            ("?", "?"),
            ("-", "-"),
            ("<Space>", "Space"),
            ("<CR>", "CR"),
            ("<Esc>", "Esc"),
            ("<BS>", "BS"),
            ("<Tab>", "Tab"),
            ("<Left>", "←"),
            ("<Right>", "→"),
            ("<Up>", "↑"),
            ("<Down>", "↓"),
            ("<C-d>", "C-d"),
            ("<S-Left>", "S-←"),
            ("<C-S-Left>", "C-S-←"),
            ("<C-->", "C--"),
        ];
        for (raw, want) in cases {
            assert_eq!(chip_text(KeyChord::parse(raw)?), want, "记法 `{raw}`");
        }
        Ok(())
    }
}
