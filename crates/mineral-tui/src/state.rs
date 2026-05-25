//! 应用全局状态。`tracks_cache` 中的「key 不存在」== 还没拉到(渲染 "loading…")。

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::DynamicImage;
use mineral_model::{MediaUrl, PlayUrl, PlaylistId, Song, SongId, SourceKind};
use mineral_spectrum::SpectrumComputer;
use mineral_task::TaskEvent;
use ratatui_image::protocol::StatefulProtocol;
use rustc_hash::{FxHashMap, FxHashSet};

use mineral_model::{LrcLyric, Lyrics, WordLyric};

use crate::components::spectrum::SpectrumState;
use crate::playback::Playback;
use crate::view_model::{PlaylistView, SongView};

/// 一条 cover protocol 缓存项:`(协议, 上次渲染时的目标 cells dims)`。
///
/// dims 用于 invalidation —— 跟当前 area 不一致就重建 protocol,避免字号 / 终端
/// 大小变了之后图按旧 dims 绘出来溢出 / 截断。
pub type CoverProtocolEntry = (StatefulProtocol, (u16, u16));

/// 左栏当前展示的视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// 歌单列表(默认)。
    Playlists,

    /// 已选歌单内的曲目列表。
    Library,
}

/// 歌词面板的副歌词显示档(翻译 / 罗马音),由 `t` 键循环切换。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LyricExtra {
    /// 只显示原文(默认)。
    #[default]
    None,

    /// 原文下叠加行级翻译。
    Translation,

    /// 原文下叠加行级罗马音。
    Romanization,
}

/// 应用顶层状态。
#[allow(dead_code)] // reason: side_scroll / lib_scroll 在阶段 7 搜索过滤时会被读取
pub struct AppState {
    /// 左栏当前视图。
    pub view: View,

    /// 已加载的歌单(跨 channel 合并;按到达顺序 append)。
    pub playlists: Vec<PlaylistView>,

    /// 歌单 id → 曲目;不在 map 里表示还没拉到。
    pub tracks_cache: FxHashMap<PlaylistId, Vec<SongView>>,

    /// 已提交过 `PlaylistTracks` 请求的歌单(成败都记)。prefetch 据此去重,
    /// 避免**失败**歌单(tracks_cache 永远不会被填)被每帧无限重提交而刷屏。
    /// 对齐 cover 的 `cover_pending`。
    pub tracks_requested: FxHashSet<PlaylistId>,

    /// 歌曲 id → 完整结构化歌词(原文 / 逐字 / 翻译 / 罗马音);不在 map 里表示还没拉到 /
    /// 拉失败。channel 层已清洗,client 直接收整份,渲染时按需取各字段。
    pub lyrics_cache: FxHashMap<SongId, Lyrics>,

    /// 副歌词(翻译 / 罗马音)显示档,由 `t` 键循环。
    pub lyric_extra: LyricExtra,

    /// Playlists 视图当前选中行。
    pub sel_playlist: usize,

    /// Playlists 视图垂直滚动偏移。
    pub side_scroll: usize,

    /// Library 视图当前选中行。
    pub sel_track: usize,

    /// Library 视图垂直滚动偏移。
    pub lib_scroll: usize,

    /// 搜索关键字。
    pub search_q: String,

    /// 当前正在播放(用于 Library 视图行首 ♫ 标记)。
    pub current: Option<Song>,

    /// 播放状态机。
    pub playback: Playback,

    /// 频谱状态(条高 + 平滑)。
    pub spectrum: SpectrumState,

    /// 频谱 FFT 计算器:吃 PCM 样本,出 64 根条的目标高度。
    pub fft: SpectrumComputer,

    /// 浮动 queue 当前曲目列表(后端权威态)。
    pub queue: Vec<Song>,

    /// 是否处于搜索输入态(`/` 触发,Enter / Esc 退出)。
    pub search_mode: bool,

    /// 预拉的下一首 PlayUrl(auto-next prefetch)。曲终瞬间命中就免去 SongUrl 等待。
    /// 不命中(用户切到别的歌 / 模式变了)就丢。`PlayUrl.song_id` 自带,不再额外打元组。
    pub prefetched: Option<PlayUrl>,

    /// Shuffle 状态下保存的原始 queue 顺序。退 Shuffle 时还原。
    /// 非 Shuffle 状态恒为 `None`。
    pub original_queue: Option<Vec<Song>>,

    /// 已拉好的封面原始图(URL → 解码后的 RGB 像素)。session 内一直留。
    pub cover_cache: FxHashMap<MediaUrl, Arc<DynamicImage>>,

    /// 在飞 fetch 集合,用于 dedup tick 重复请求。
    pub cover_pending: FxHashSet<MediaUrl>,

    /// 渲染用的 ratatui-image stateful protocol 缓存。`StatefulProtocol` 内部记编码状态
    /// (kitty 的图片 id、sixel 编码缓冲等),render 复用就不会每帧重发图。
    /// 用 `RefCell` 是因为 `view::draw` 拿 `&AppState`,而 stateful_widget 渲染要 `&mut`。
    pub cover_protocols: RefCell<FxHashMap<MediaUrl, CoverProtocolEntry>>,

    /// 后台 server scheduler 当前快照(每 tick 由 App 从 `Client::task_snapshot`
    /// 灌入)。**只含**:server 端 ChannelFetch lane(playlists / tracks /
    /// song-url / lyrics / liked)。封面是 client-local 的 [`CoverFetcher`],
    /// 不在这里——见 [`Self::cover_loading`]。
    /// `by_kind` 给 top_status 显示「pl:N tr:N ...」按 kind 拆分用。
    pub tasks_snapshot: mineral_task::Snapshot,

    /// 当前 client-side cover_fetcher in-flight 数(等价 `cover_pending.len()`,
    /// 每 tick 由 App 灌入)。
    pub cover_loading: usize,

    /// 各 channel 当前用户喜欢(♥)的歌曲 ID 集合;装饰 `SongView.loved` 用。
    /// 缺 source 时该 source 的歌全部按 `loved=false` 渲染。
    pub liked_ids: FxHashMap<SourceKind, FxHashSet<SongId>>,

    /// 上一次选中行变化的时间(navigation key 命中时刷新)。cover_image 用它做
    /// 防抖:连续滚动时跳过昂贵的 protocol 构建,稳态后再上图。
    pub last_sel_change: Instant,
}

/// 选中变化后多久才允许 cover_image 构建新 protocol。期间走程序化 fallback,稳态后再切真图。
pub const COVER_DEBOUNCE: Duration = Duration::from_millis(80);

impl AppState {
    /// 构造空状态。所有列表 / 缓存初始为空,等 [`AppState::apply`] 增量填充。
    pub fn empty() -> Self {
        Self {
            view: View::Playlists,
            playlists: Vec::new(),
            tracks_cache: FxHashMap::default(),
            tracks_requested: FxHashSet::default(),
            lyrics_cache: FxHashMap::default(),
            lyric_extra: LyricExtra::None,
            sel_playlist: 0,
            side_scroll: 0,
            sel_track: 0,
            lib_scroll: 0,
            search_q: String::new(),
            current: None,
            playback: Playback::new(),
            spectrum: SpectrumState::new(),
            fft: SpectrumComputer::new(),
            queue: Vec::new(),
            search_mode: false,
            prefetched: None,
            original_queue: None,
            cover_cache: FxHashMap::default(),
            cover_pending: FxHashSet::default(),
            cover_protocols: RefCell::new(FxHashMap::default()),
            tasks_snapshot: mineral_task::Snapshot {
                running: 0,
                by_lane: FxHashMap::default(),
                by_kind: FxHashMap::default(),
            },
            cover_loading: 0,
            liked_ids: FxHashMap::default(),
            last_sel_change: Instant::now(),
        }
    }

    /// 距上次选中变化是否仍在 [`COVER_DEBOUNCE`] 防抖窗口内。
    pub fn is_scrolling(&self) -> bool {
        self.last_sel_change.elapsed() < COVER_DEBOUNCE
    }

    /// 给定一首歌,根据当前 `liked_ids` / 未来其他 user-data 装饰成 SongView。
    /// 这是 user-data 写入 SongView 的**唯一入口**;新增 user-data 字段时只改这里。
    fn decorate(&self, song: Song) -> SongView {
        let loved = self
            .liked_ids
            .get(&song.source())
            .is_some_and(|s| s.contains(&song.id));
        SongView {
            data: song,
            loved,
            plays: 0,
        }
    }

    /// 某个 channel 的 user-data 到位 / 变化时,把 `tracks_cache` 里属于该 source
    /// 的 SongView 全部按当前 `decorate` 重建一遍。
    /// 跨 source 的歌单不动(decoration data 是 per-source 的)。
    fn redecorate_for_source(&mut self, source: SourceKind) {
        let cache = std::mem::take(&mut self.tracks_cache);
        self.tracks_cache = cache
            .into_iter()
            .map(|(pid, tracks)| {
                let next: Vec<SongView> = tracks
                    .into_iter()
                    .map(|sv| {
                        if sv.data.source() == source {
                            self.decorate(sv.data)
                        } else {
                            sv
                        }
                    })
                    .collect();
                (pid, next)
            })
            .collect();
    }

    /// 把任务事件应用到状态。**4c 后**:server 端 PlayerCore 已 filter 掉
    /// `PlayUrlReady` / `LyricsReady`(自己消化进 PlayerSnapshot),client 这里
    /// 只剩 playlists / tracks / liked_ids 三类。
    pub fn apply(&mut self, event: &TaskEvent) {
        match event {
            TaskEvent::PlaylistsFetched { playlists, .. } => {
                self.playlists
                    .extend(playlists.iter().cloned().map(|data| PlaylistView { data }));
                if self.sel_playlist >= self.playlists.len() {
                    self.sel_playlist = 0;
                }
            }
            TaskEvent::PlaylistTracksFetched { id, tracks } => {
                let decorated = tracks
                    .iter()
                    .cloned()
                    .map(|data| self.decorate(data))
                    .collect();
                self.tracks_cache.insert(id.clone(), decorated);
            }
            TaskEvent::LikedSongIdsFetched { source, ids } => {
                self.liked_ids.insert(*source, ids.clone());
                self.redecorate_for_source(*source);
            }
            // server 已 filter,理论不会到 client。defensive:跳过。
            TaskEvent::PlayUrlReady { .. } | TaskEvent::LyricsReady { .. } => {}
        }
    }

    /// 当前曲目的完整歌词集合;未拉到时返回 `None`。
    fn current_lyrics_set(&self) -> Option<&Lyrics> {
        let song = self.playback.track.as_ref()?;
        self.lyrics_cache.get(&song.id)
    }

    /// 当前曲目的行级歌词(按时间升序);未拉到时返回 `None`。
    pub fn current_lyrics(&self) -> Option<&LrcLyric> {
        self.current_lyrics_set().map(|l| &l.lrc)
    }

    /// 当前曲目的逐字歌词;无逐字(channel 未返回)时返回 `None`。
    pub fn current_words(&self) -> Option<&WordLyric> {
        self.current_lyrics_set().map(|l| &l.words)
    }

    /// 当前曲目的行级翻译;未拉到时返回 `None`。
    pub fn current_translation(&self) -> Option<&LrcLyric> {
        self.current_lyrics_set().map(|l| &l.translation)
    }

    /// 当前曲目的行级罗马音;未拉到时返回 `None`。
    pub fn current_romanization(&self) -> Option<&LrcLyric> {
        self.current_lyrics_set().map(|l| &l.romanization)
    }

    /// 当前曲目是否有任一副歌词(翻译 / 罗马音)可切换。无则歌词面板不显示 `t` 提示。
    pub fn has_extra_lyrics(&self) -> bool {
        self.current_translation().is_some_and(|l| !l.is_empty())
            || self.current_romanization().is_some_and(|l| !l.is_empty())
    }

    /// 当前档对应的副歌词(`None` 档 / 该档为空都返回 `None`)。
    pub fn current_extra_lyric(&self) -> Option<&LrcLyric> {
        let extra = match self.lyric_extra {
            LyricExtra::None => return None,
            LyricExtra::Translation => self.current_translation(),
            LyricExtra::Romanization => self.current_romanization(),
        };
        extra.filter(|l| !l.is_empty())
    }

    /// 循环副歌词档:`None → Translation → Romanization → None`,跳过当前歌为空的档。
    /// 翻译 / 罗马音都缺时停在 `None`。
    pub fn cycle_lyric_extra(&mut self) {
        let has_trans = self.current_translation().is_some_and(|l| !l.is_empty());
        let has_roma = self.current_romanization().is_some_and(|l| !l.is_empty());
        self.lyric_extra = match self.lyric_extra {
            LyricExtra::None if has_trans => LyricExtra::Translation,
            LyricExtra::None if has_roma => LyricExtra::Romanization,
            LyricExtra::None => LyricExtra::None,
            LyricExtra::Translation if has_roma => LyricExtra::Romanization,
            LyricExtra::Translation => LyricExtra::None,
            LyricExtra::Romanization => LyricExtra::None,
        };
    }

    /// 返回当前选中歌单的引用。
    ///
    /// `sel_playlist` 的语义随 [`Self::view`] 切换:
    /// - Playlists 视图:filtered 列表的索引,过滤词作用于 playlist 名,渲染、导航、
    ///   selected_playlist 都对齐 filtered。
    /// - Library 视图:raw 列表的索引(进 Library 时已 remap 锁定为「用户进的那条」),
    ///   此时 search_q 作用于 tracks,跟 playlists 过滤无关。
    pub fn selected_playlist(&self) -> Option<&PlaylistView> {
        match self.view {
            View::Playlists => self.filtered_playlists().get(self.sel_playlist).copied(),
            View::Library => self.playlists.get(self.sel_playlist),
        }
    }

    /// 当前选中歌单的曲目槽位(`None` = 还没拉到)。
    pub fn current_tracks_slot(&self) -> Option<&Vec<SongView>> {
        self.selected_playlist()
            .and_then(|p| self.tracks_cache.get(&p.data.id))
    }

    /// 当前选中歌单的曲目列表(slot 未到位时返回空)。
    pub fn current_tracks(&self) -> Vec<SongView> {
        self.current_tracks_slot().cloned().unwrap_or_default()
    }

    /// 给定歌单的总时长(ms);槽位未到位时返回 0。
    pub fn total_duration_ms_of(&self, id: &PlaylistId) -> u64 {
        self.tracks_cache
            .get(id)
            .map(|tracks| tracks.iter().map(|sv| sv.data.duration_ms).sum())
            .unwrap_or(0)
    }

    /// 当前在播歌在 queue 中的下标(打开浮层时把光标定位到此)。无在播曲返回 `None`。
    pub fn queue_current_index(&self) -> Option<usize> {
        let id = &self.playback.track.as_ref()?.id;
        self.queue.iter().position(|s| &s.id == id)
    }

    /// 当前可见(被 search 过滤)的歌单列表。
    pub fn filtered_playlists(&self) -> Vec<&PlaylistView> {
        if self.search_q.is_empty() {
            self.playlists.iter().collect()
        } else {
            let q = self.search_q.to_lowercase();
            self.playlists
                .iter()
                .filter(|p| p.data.name.to_lowercase().contains(&q))
                .collect()
        }
    }

    /// 当前可见(被 search 过滤)的曲目列表。
    pub fn filtered_tracks(&self) -> Vec<SongView> {
        let tracks = self.current_tracks();
        if self.search_q.is_empty() {
            tracks
        } else {
            let q = self.search_q.to_lowercase();
            tracks
                .into_iter()
                .filter(|sv| {
                    sv.data.name.to_lowercase().contains(&q)
                        || sv
                            .data
                            .artists
                            .iter()
                            .any(|a| a.name.to_lowercase().contains(&q))
                        || sv
                            .data
                            .album
                            .as_ref()
                            .is_some_and(|a| a.name.to_lowercase().contains(&q))
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::endserenading;

    use super::AppState;

    /// `queue_current_index` 命中在播歌下标;无在播曲返回 `None`。
    #[test]
    fn queue_current_index_finds_playing() {
        let mut s = AppState::empty();
        let queue = endserenading(5);
        s.playback.track = queue.get(2).cloned();
        s.queue = queue;
        assert_eq!(s.queue_current_index(), Some(2));

        s.playback.track = None;
        assert_eq!(s.queue_current_index(), None);
    }
}
