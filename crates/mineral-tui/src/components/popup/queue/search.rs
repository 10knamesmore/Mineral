//! queue 浮层的 `/` 模糊过滤:显示层重排 + 命中高亮 + 输入态键处理。
//!
//! 过滤只动**显示**([`QueueOverlay::visible`] 给出按匹配分降序的队列真实下标),
//! 底层播放队列不发任何编辑。输入态键处理与浏览页 `/` 同构:Enter 保留词退输入、
//! Esc 清词退出、逐字改词把光标复位到最相关行。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_model::Song;
use ratatui::style::Style;
use ratatui::text::Span;

use super::footer::position_label;
use super::overlay::QueueOverlay;
use super::row::RowHits;
use crate::components::popup::component::OverlayResponse;
use crate::render::cursor::cursor_spans;
use crate::render::theme::Theme;
use crate::runtime::line_input::InputRequest;
use crate::runtime::state::{AppState, SearchState};

impl QueueOverlay {
    /// 当前过滤词是否非空(过滤生效中)。
    pub(super) fn is_filtering(&self) -> bool {
        !self.search.query().is_empty()
    }

    /// 是否处于 `/` 输入态(逐字改词)。
    pub(super) fn is_typing(&self) -> bool {
        self.search.typing
    }

    /// 队列过滤视图:给出**队列真实下标**序列。
    ///
    /// 无过滤词 → 恒等 `0..len`(保持播放顺序)。有词 → 命中歌名 / 别名 / 任一艺人 /
    /// 专辑名取最高分,按分降序(等分保持队列原序,`sort_by_key` 稳定排序)。
    /// 「最接近输入」在顶,与浏览页曲目过滤同序。
    pub(super) fn visible(&self, ctx: &AppState) -> Vec<usize> {
        let queue = &ctx.player.queue;
        if !self.is_filtering() {
            return (0..queue.len()).collect();
        }
        let mut scored: Vec<(u32, usize)> = queue
            .iter()
            .enumerate()
            .filter_map(|(i, s)| song_score(&self.search, s).map(|sc| (sc, i)))
            .collect();
        scored.sort_by_key(|&(sc, _)| std::cmp::Reverse(sc));
        scored.into_iter().map(|(_, i)| i).collect()
    }

    /// 过滤视图当前光标对应的**队列真实下标**;空视图 → `None`。
    ///
    /// 脚本 ctx / cursor 记忆据此取选中歌——不能用过滤视图位(重排后会错指)。
    pub(crate) fn raw_cursor(&self, ctx: &AppState) -> Option<usize> {
        self.visible(ctx).get(self.list.sel()).copied()
    }

    /// 进入 `/` 输入态并清词,光标归首。
    pub(super) fn begin_search(&mut self) {
        self.search.typing = true;
        self.search.clear();
        self.list.set_sel(0);
    }

    /// 清掉过滤词、退出过滤:光标落回原选中歌的队列真实下标(词清后视图恒等,
    /// 过滤视图位即真实位)。
    pub(super) fn clear_filter(&mut self, ctx: &AppState) {
        let raw = self.raw_cursor(ctx).unwrap_or(0);
        self.search.typing = false;
        self.search.clear();
        self.list.set_sel(raw);
    }

    /// `/` 输入态按键:与浏览页 `/` 同构。改词后光标复位到最相关行(过滤视图首行)。
    /// 输入态吞掉一切键(含空格),不半穿透给全局播放控制。
    ///
    /// 带 CONTROL 的字符键一律吞掉——否则 `<C-d>` 族滚动键会把裸字符塞进 query。
    pub(super) fn on_search_key(&mut self, key: &KeyEvent) -> OverlayResponse {
        match key.code {
            KeyCode::Esc => {
                self.search.typing = false;
                self.search.clear();
                self.list.set_sel(0);
            }
            KeyCode::Enter => self.search.typing = false,
            KeyCode::Backspace => {
                if self.search.query().is_empty() {
                    self.search.typing = false;
                } else if self.search.edit(InputRequest::DeletePrev) {
                    self.list.set_sel(0);
                }
            }
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(c) => {
                self.search.edit(InputRequest::Insert(c));
                self.list.set_sel(0);
            }
            KeyCode::Left => {
                self.search.edit(InputRequest::Left);
            }
            KeyCode::Right => {
                self.search.edit(InputRequest::Right);
            }
            KeyCode::Home => {
                self.search.edit(InputRequest::Home);
            }
            KeyCode::End => {
                self.search.edit(InputRequest::End);
            }
            _ => {}
        }
        OverlayResponse::Consumed
    }

    /// 顶栏标题里的 `/query` 输入片段(输入态额外画反色文本光标);无过滤 → 空。
    /// 与浏览页顶栏 ` playlists /query` 同位——输入框在上,不在底栏。
    pub(super) fn search_input(&self, theme: &Theme) -> Vec<Span<'static>> {
        if !self.is_typing() && !self.is_filtering() {
            return Vec::new();
        }
        let accent = Style::new().fg(theme.peach);
        if self.is_typing() {
            let (before, after) = self.search.query_split();
            cursor_spans(format!("/{before}"), after, accent)
        } else {
            vec![Span::styled(format!("/{}", self.search.query()), accent)]
        }
    }

    /// 底栏左下位置标签:过滤态给 `命中位 / 命中数`(告诉你在第几个命中、共几个),
    /// 否则 `n / total`。
    pub(super) fn position_bottom(&self, ctx: &AppState) -> String {
        if !self.is_filtering() {
            return position_label(self.list.sel(), ctx);
        }
        let matches = self.visible(ctx).len();
        let at = if matches == 0 {
            0
        } else {
            self.list.sel().saturating_add(1).min(matches)
        };
        format!(" {at} / {matches} ")
    }

    /// 一行各文本列的命中下标(渲染高亮用);无过滤词 → 全空(`highlight_indices` 退化原样)。
    /// 每列独立匹配(与打分同口径),故别名命中也能高亮,不因它是后缀而漏掉。
    pub(super) fn row_hits(&self, song: &Song) -> RowHits {
        if !self.is_filtering() {
            return RowHits::default();
        }
        RowHits {
            name: self.field_hits(&song.name),
            alias: song
                .alias
                .as_deref()
                .map(|a| self.field_hits(a))
                .unwrap_or_default(),
            artist: song
                .artists
                .first()
                .map(|a| self.field_hits(&a.name))
                .unwrap_or_default(),
            album: song
                .album
                .as_ref()
                .map(|a| self.field_hits(&a.name))
                .unwrap_or_default(),
        }
    }

    /// 单个字段对当前过滤词的命中 char 下标(已映射回原文);不命中 / 空词 → 空。
    fn field_hits(&self, text: &str) -> smallvec::SmallVec<[u32; 8]> {
        self.search
            .match_for(text)
            .map(|m| m.hits)
            .unwrap_or_default()
    }
}

/// 一首歌对当前过滤词的最高匹配分:歌名 / 别名 / 任一艺人 / 专辑名取最高;无命中 → `None`。
/// 与浏览页曲目过滤同口径(展示了别名就得能搜到它,故别名独立一段匹配)。
fn song_score(search: &SearchState, song: &Song) -> Option<u32> {
    let name = search.match_for(&song.name).map(|m| m.score);
    let alias = song
        .alias
        .as_deref()
        .and_then(|a| search.match_for(a).map(|m| m.score));
    let artist = song
        .artists
        .iter()
        .filter_map(|a| search.match_for(&a.name).map(|m| m.score))
        .max();
    let album = song
        .album
        .as_ref()
        .and_then(|a| search.match_for(&a.name).map(|m| m.score));
    name.into_iter()
        .chain(alias)
        .chain(artist)
        .chain(album)
        .max()
}
