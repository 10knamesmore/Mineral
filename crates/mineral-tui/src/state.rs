//! 应用全局状态。`tracks_cache` 中的「key 不存在」== 还没拉到(渲染 "loading…")。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use image::DynamicImage;
use mineral_model::{MediaUrl, PlaylistId, Song, SongId, SourceKind};
use mineral_spectrum::SpectrumComputer;
use mineral_task::TaskEvent;
use ratatui_image::protocol::StatefulProtocol;

use crate::lrc;
use crate::yrc::{self, YrcLine};

use crate::components::spectrum::SpectrumState;
use crate::playback::Playback;
use crate::view_model::{PlaylistView, SongView};

/// 左栏当前展示的视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// 歌单列表(默认)。
    Playlists,

    /// 已选歌单内的曲目列表。
    Library,
}

/// 当前键盘焦点(用于浮层与主区域之间路由按键)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    /// 主区域(左栏 / library)。
    #[default]
    Left,

    /// 浮动 queue 面板。
    Queue,
}

/// 应用顶层状态。
#[allow(dead_code)] // reason: side_scroll / lib_scroll 在阶段 7 搜索过滤时会被读取
pub struct AppState {
    /// 左栏当前视图。
    pub view: View,

    /// 已加载的歌单(跨 channel 合并;按到达顺序 append)。
    pub playlists: Vec<PlaylistView>,

    /// 歌单 id → 曲目;不在 map 里表示还没拉到。
    pub tracks_cache: HashMap<PlaylistId, Vec<SongView>>,

    /// 歌曲 id → 解析后的 LRC 行;不在 map 里表示还没拉到 / 拉失败。
    pub lyrics_cache: HashMap<SongId, Vec<(u64, String)>>,

    /// 歌曲 id → 解析后的 YRC(逐字)行;有 yrc 才插入,渲染时优先于 LRC。
    pub yrc_cache: HashMap<SongId, Vec<YrcLine>>,

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

    /// 浮动 queue 当前曲目列表。
    pub queue: Vec<Song>,

    /// queue 浮层是否显示。
    pub queue_open: bool,

    /// queue 浮层选中行。
    pub queue_sel: usize,

    /// 当前键盘焦点。
    pub focus: Focus,

    /// 是否处于搜索输入态(`/` 触发,Enter / Esc 退出)。
    pub search_mode: bool,

    /// quit confirm modal 是否打开。
    pub confirm_open: bool,

    /// 预拉的下一首播放 URL(auto-next prefetch)。曲终瞬间命中就免去 SongUrl 等待。
    /// 不命中(用户切到别的歌 / 模式变了)就丢。
    pub prefetched: Option<(SongId, MediaUrl)>,

    /// Shuffle 状态下保存的原始 queue 顺序。退 Shuffle 时还原。
    /// 非 Shuffle 状态恒为 `None`。
    pub original_queue: Option<Vec<Song>>,

    /// 已拉好的封面原始图(URL → 解码后的 RGB 像素)。session 内一直留。
    pub cover_cache: HashMap<MediaUrl, Arc<DynamicImage>>,

    /// 在飞 fetch 集合,用于 dedup tick 重复请求。
    pub cover_pending: HashSet<MediaUrl>,

    /// 渲染用的 ratatui-image stateful protocol 缓存。`StatefulProtocol` 内部记编码状态
    /// (kitty 的图片 id、sixel 编码缓冲等),render 复用就不会每帧重发图。
    /// 用 `RefCell` 是因为 `view::draw` 拿 `&AppState`,而 stateful_widget 渲染要 `&mut`。
    pub cover_protocols: RefCell<HashMap<MediaUrl, StatefulProtocol>>,

    /// 后台 scheduler 当前 running 任务数(每 tick 由 App 从 `Scheduler::snapshot` 灌入)。
    /// 给 top_status 显示「↓N」用,直观看到封面 / 歌词 / playlist 拉取进度。
    pub tasks_running: usize,

    /// 各 channel 当前用户喜欢(♥)的歌曲 ID 集合;装饰 `SongView.loved` 用。
    /// 缺 source 时该 source 的歌全部按 `loved=false` 渲染。
    pub liked_ids: HashMap<SourceKind, HashSet<SongId>>,
}

impl AppState {
    /// 构造空状态。所有列表 / 缓存初始为空,等 [`AppState::apply`] 增量填充。
    pub fn empty() -> Self {
        Self {
            view: View::Playlists,
            playlists: Vec::new(),
            tracks_cache: HashMap::new(),
            lyrics_cache: HashMap::new(),
            yrc_cache: HashMap::new(),
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
            queue_open: false,
            queue_sel: 0,
            focus: Focus::Left,
            search_mode: false,
            confirm_open: false,
            prefetched: None,
            original_queue: None,
            cover_cache: HashMap::new(),
            cover_pending: HashSet::new(),
            cover_protocols: RefCell::new(HashMap::new()),
            tasks_running: 0,
            liked_ids: HashMap::new(),
        }
    }

    /// 给定一首歌,根据当前 `liked_ids` / 未来其他 user-data 装饰成 SongView。
    /// 这是 user-data 写入 SongView 的**唯一入口**;新增 user-data 字段时只改这里。
    fn decorate(&self, song: Song) -> SongView {
        let loved = self
            .liked_ids
            .get(&song.source)
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
                        if sv.data.source == source {
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

    /// 把任务事件应用到状态(只更新 UI 数据,fan-out 副作用由 [`crate::app::App`] 负责)。
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
            // 由 App 直接 forward 给 audio,state 不存 url。
            TaskEvent::PlayUrlReady { .. } => {}
            TaskEvent::LyricsReady { song_id, lyrics } => {
                // 翻译 / 罗马音的 UI 切换留 backlog。空 LRC 也存空 vec,让渲染层走「无歌词」
                // 分支(避免反复重试)。yrc 仅在网易返回非空时插入,渲染时优先 yrc 兜底 lrc。
                let parsed_lrc = lyrics
                    .lrc
                    .as_deref()
                    .map(lrc::parse_lrc)
                    .unwrap_or_default();
                self.lyrics_cache.insert(song_id.clone(), parsed_lrc);
                if let Some(raw_yrc) = lyrics.yrc.as_deref() {
                    let parsed_yrc = yrc::parse_yrc(raw_yrc);
                    if !parsed_yrc.is_empty() {
                        self.yrc_cache.insert(song_id.clone(), parsed_yrc);
                    }
                }
            }
            TaskEvent::CoverReady { url, image } => {
                self.cover_pending.remove(url);
                self.cover_cache.insert(url.clone(), Arc::clone(image));
                // 如果之前已经为这张图建过 protocol(罕见),把缓存清掉,下次渲染重建
                // —— 防止 cache miss 后又来到 CoverReady 导致旧 protocol 跟新图 desync。
                self.cover_protocols.borrow_mut().remove(url);
            }
        }
    }

    /// 当前曲目的歌词行(已解析按时间升序);未拉到时返回 `None`。
    pub fn current_lyrics(&self) -> Option<&Vec<(u64, String)>> {
        let song = self.playback.track.as_ref()?;
        self.lyrics_cache.get(&song.id)
    }

    /// 当前曲目的 YRC 逐字行;无 yrc(网易未返回 / 非网易源)时返回 `None`。
    pub fn current_yrc(&self) -> Option<&Vec<YrcLine>> {
        let song = self.playback.track.as_ref()?;
        self.yrc_cache.get(&song.id)
    }

    /// 返回当前选中歌单(Playlists 视图)的引用。
    pub fn selected_playlist(&self) -> Option<&PlaylistView> {
        self.playlists.get(self.sel_playlist)
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
                            .first()
                            .is_some_and(|a| a.name.to_lowercase().contains(&q))
                })
                .collect()
        }
    }
}
