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
use tokio::sync::mpsc;

use mineral_model::{LrcLyric, Lyrics, WordLyric};

use crate::components::layout::spectrum::SpectrumState;
use crate::render::anim::{Transition, ticks16_from_ms};
use crate::render::palette::CoverPalette;
use crate::runtime::cover_encode::EncodeRequest;
use crate::runtime::filter::{FuzzyMatcher, Match, MatchableText};
use crate::runtime::playback::Playback;
use crate::runtime::view_model::{PlaylistView, SongView};

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

impl LyricExtra {
    /// 稳定字符串名(UI 偏好持久化用),与 [`Self::from_name`] 对偶。
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Translation => "translation",
            Self::Romanization => "romanization",
        }
    }

    /// 从 [`Self::name`] 的稳定名解析回来。
    ///
    /// # Params:
    ///   - `name`: 稳定名字符串(落库值)
    ///
    /// # Return:
    ///   对应档位;未知名(脏数据)为 `None`,调用方降级到默认档。
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "translation" => Some(Self::Translation),
            "romanization" => Some(Self::Romanization),
            _ => None,
        }
    }
}

/// 应用顶层状态。
#[allow(dead_code)] // reason: side_scroll / lib_scroll 在阶段 7 搜索过滤时会被读取
pub struct AppState {
    /// 左栏当前视图。切换时立即设为目标值,供按键路由;渲染端的过渡位置看 [`Self::view_pos`]。
    pub view: View,

    /// 左栏 Playlists ↔ Library 横向过渡位置:`0` = Playlists、满值 = Library。
    /// 切到 Library 调 `enter`、回 Playlists 调 `leave`,中途再反向只改 target 不跳变。
    pub view_pos: Transition,

    /// 是否处于全屏播放态。切换时立即设为目标值供按键路由;渲染端的形变进度看 [`Self::fullscreen_pos`]。
    pub fullscreen: bool,

    /// 全屏播放进退场形变进度:`0` = 浏览态、满值 = 全屏。进调 `enter`、退调 `leave`,
    /// 中途再反向只改 target 不跳变(与 [`Self::view_pos`] 同范式)。
    pub fullscreen_pos: Transition,

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

    /// 脚本下发的 session 级旋钮覆盖(`Event::UiOverride` 落地;渲染处
    /// 有覆盖读覆盖、无覆盖读配置)。
    pub ui_overrides: crate::runtime::ui_override::UiOverrides,

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

    /// 上次已应用的 server 状态版本号(每 tick 随 PlayerSync 回报;0 = 还没同步过,
    /// 首次同步必然全量)。
    pub versions: mineral_protocol::PlayerVersions,

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

    /// 已取色的封面色板(URL → 频谱 2D 色场的重点色,Lab 明度升序)。
    /// 缺 key = 没取到色(取色失败 / 还没回传)。session 内一直留,顺手缓存复用。
    pub cover_palettes: FxHashMap<MediaUrl, CoverPalette>,

    /// 上次已应用到频谱的封面 key(频谱当前色场对应哪张封面)。
    /// `None` = 频谱在 hue 漂移(无封面 / 取色未就绪)。`sync_spectrum_palette` 身份判定用。
    pub spectrum_cover: Option<MediaUrl>,

    /// 在飞 fetch 集合,用于 dedup tick 重复请求。
    pub cover_pending: FxHashSet<MediaUrl>,

    /// 渲染用的 ratatui-image stateful protocol 缓存。`StatefulProtocol` 内部记编码状态
    /// (kitty 的图片 id、sixel 编码缓冲等),render 复用就不会每帧重发图。
    /// 用 `RefCell` 是因为 `view::draw` 拿 `&AppState`,而 stateful_widget 渲染要 `&mut`。
    pub cover_protocols: RefCell<FxHashMap<MediaUrl, CoverProtocolEntry>>,

    /// 封面编码请求发送端(投递给 [`crate::runtime::cover_encode::CoverEncoder`] 的 worker)。
    /// 渲染处未命中已编码协议时投一次,把 resize + base64 编码挪出渲染线程。禁用态(测试 /
    /// 无 runtime)是个无接收端的 sender,投递静默丢弃。
    pub cover_encode_tx: mpsc::UnboundedSender<EncodeRequest>,

    /// 在飞编码 `(URL, 维度)` 集合,渲染处据此 dedup —— 同一封面同尺寸只投一次,等结果回填。
    /// 用 `RefCell` 因渲染拿 `&AppState`。
    pub cover_encode_pending: RefCell<FxHashSet<(MediaUrl, (u16, u16))>>,

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

    /// 歌曲 id → 远端真实累计播放次数;装饰 `SongView.plays` 用。
    /// 缺 id = 还没查到 / 查失败(渲染成 `None`)。
    pub play_counts: FxHashMap<SongId, u32>,

    /// 已提交过 `RemotePlayCount` 请求的歌曲(成败都记)。停留防抖据此去重,
    /// 避免同一首歌反复打回忆坐标接口。
    pub play_count_requested: FxHashSet<SongId>,

    /// 上一次选中行变化的时间(navigation key 命中时刷新)。cover_image 用它做
    /// 防抖:连续滚动时跳过昂贵的 protocol 构建,稳态后再上图。
    pub last_sel_change: Instant,

    /// 本地搜索的模糊匹配器(fzf 风格子序列 + 中文拼音/首字母联合)。
    /// `&self` 路径下要复用 buffer,因此包 `RefCell`,与 `cover_protocols` 同理。
    pub matcher: RefCell<FuzzyMatcher>,

    /// 文本 → 预处理 [`MatchableText`] 的缓存。键是原始文本(歌名 / 艺人名 / 专辑名 /
    /// 歌单名),session 内长留;规模(每条 ~几百字节,总量上限 ≈ 已加载曲目数 × 3)
    /// 远低于其它 cache。换源 / 重启自然清掉。
    pub matchable_cache: RefCell<FxHashMap<String, Arc<MatchableText>>>,

    /// 已加载的全局配置(`Arc` 共享只读):渲染 / 运行时模块经此读各段旋钮
    /// (lyrics 行距、layout 阈值、prefetch 半径、animation 时长等)。
    pub cfg: Arc<mineral_config::Config>,
}

impl AppState {
    /// 构造空状态(所有列表 / 缓存初始为空,等 [`AppState::apply`] 增量填充);
    /// 过渡时长 / 频谱旋钮 / 各段手感由注入的配置落地。
    ///
    /// # Params:
    ///   - `cfg`: 已加载的全局配置(`Arc` 共享,渲染/运行时模块经 `state.cfg` 读)
    pub fn new(cfg: Arc<mineral_config::Config>) -> Self {
        let anim = cfg.tui().animation();
        let tick_ms = *anim.frame_tick_ms();
        Self {
            view: View::Playlists,
            view_pos: Transition::new(ticks16_from_ms(*anim.sweep_ms(), tick_ms)),
            fullscreen: false,
            fullscreen_pos: Transition::new(ticks16_from_ms(*anim.fullscreen_ms(), tick_ms)),
            playlists: Vec::new(),
            tracks_cache: FxHashMap::default(),
            tracks_requested: FxHashSet::default(),
            lyrics_cache: FxHashMap::default(),
            lyric_extra: LyricExtra::None,
            ui_overrides: crate::runtime::ui_override::UiOverrides::default(),
            sel_playlist: 0,
            side_scroll: 0,
            sel_track: 0,
            lib_scroll: 0,
            search_q: String::new(),
            current: None,
            playback: Playback::new(),
            spectrum: SpectrumState::new(cfg.tui().spectrum().clone(), tick_ms),
            fft: SpectrumComputer::new(spectrum_params(cfg.tui().spectrum())),
            queue: Vec::new(),
            versions: mineral_protocol::PlayerVersions::default(),
            search_mode: false,
            prefetched: None,
            original_queue: None,
            cover_cache: FxHashMap::default(),
            cover_palettes: FxHashMap::default(),
            spectrum_cover: None,
            cover_pending: FxHashSet::default(),
            cover_protocols: RefCell::new(FxHashMap::default()),
            // 默认禁用:无接收端的 sender(投递即丢)。真实 worker 由 `App::new` 注入
            // `CoverEncoder::sender()` 覆盖此字段。
            cover_encode_tx: mpsc::unbounded_channel().0,
            cover_encode_pending: RefCell::new(FxHashSet::default()),
            tasks_snapshot: mineral_task::Snapshot {
                running: 0,
                by_lane: FxHashMap::default(),
                by_kind: FxHashMap::default(),
            },
            cover_loading: 0,
            liked_ids: FxHashMap::default(),
            play_counts: FxHashMap::default(),
            play_count_requested: FxHashSet::default(),
            last_sel_change: Instant::now(),
            matcher: RefCell::new(FuzzyMatcher::new()),
            matchable_cache: RefCell::new(FxHashMap::default()),
            cfg,
        }
    }

    /// 测试构造:defaults 配置(= 接线前硬编码常量)的空状态。
    #[cfg(test)]
    pub(crate) fn test_default() -> color_eyre::Result<Self> {
        Ok(Self::new(Arc::new(mineral_config::Config::defaults()?)))
    }

    /// 距上次选中变化是否仍在封面 debounce 防抖窗口内(配置 `tui.cover.debounce_ms`)。
    pub fn is_scrolling(&self) -> bool {
        self.last_sel_change.elapsed()
            < Duration::from_millis(*self.cfg.tui().cover().debounce_ms())
    }

    /// 给定一首歌,根据当前 `liked_ids` / 未来其他 user-data 装饰成 SongView。
    /// 这是 user-data 写入 SongView 的**唯一入口**;新增 user-data 字段时只改这里。
    fn decorate(&self, song: Song) -> SongView {
        let loved = self
            .liked_ids
            .get(&song.source())
            .is_some_and(|s| s.contains(&song.id));
        let plays = self.play_counts.get(&song.id).copied();
        SongView {
            data: song,
            loved,
            plays,
        }
    }

    /// 本地乐观切换一首歌的喜欢态(翻转 `liked_ids` 并重装该源曲目)。
    ///
    /// 不等 server 确认——按键即时反馈;真实持久化由 `client.toggle_love` 触发,
    /// 失败由下次 `LikedSongIdsFetched` fetch 纠正。
    ///
    /// # Params:
    ///   - `song`: 目标歌曲
    pub fn toggle_loved_local(&mut self, song: &Song) {
        let set = self.liked_ids.entry(song.source()).or_default();
        if !set.remove(&song.id) {
            set.insert(song.id.clone());
        }
        self.redecorate_for_source(song.source());
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
    /// `PlayUrlReady` / `LyricsReady`(自己消化进 PlayerSync 的 current 重段),client 这里
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
            TaskEvent::RemotePlayCountFetched { song_id, count } => {
                self.play_counts.insert(song_id.clone(), *count);
                self.redecorate_for_source(song_id.namespace());
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
    ///
    /// 空 query → 原序;非空 query → fzf 风格模糊匹配(拼音/首字母也算命中),
    /// 按 score 降序排,**stable** 保证同分按原序。
    pub fn filtered_playlists(&self) -> Vec<&PlaylistView> {
        if self.search_q.is_empty() {
            return self.playlists.iter().collect();
        }
        self.sync_query();
        let mut scored: Vec<(u32, &PlaylistView)> = self
            .playlists
            .iter()
            .filter_map(|p| self.match_for(&p.data.name).map(|m| (m.score, p)))
            .collect();
        // sort_by_key 是 stable:同分项保持原序。
        scored.sort_by_key(|&(s, _)| std::cmp::Reverse(s));
        scored.into_iter().map(|(_, p)| p).collect()
    }

    /// 当前可见(被 search 过滤)的曲目列表。
    ///
    /// 命中规则:歌名 / 任一艺人 / 专辑名取最高分作为该曲分数。
    pub fn filtered_tracks(&self) -> Vec<SongView> {
        let tracks = self.current_tracks();
        if self.search_q.is_empty() {
            return tracks;
        }
        self.sync_query();
        let mut scored: Vec<(u32, SongView)> = tracks
            .into_iter()
            .filter_map(|sv| {
                let name = self.match_for(&sv.data.name).map(|m| m.score);
                let artist = sv
                    .data
                    .artists
                    .iter()
                    .filter_map(|a| self.match_for(&a.name).map(|m| m.score))
                    .max();
                let album = sv
                    .data
                    .album
                    .as_ref()
                    .and_then(|a| self.match_for(&a.name).map(|m| m.score));
                let best = name.into_iter().chain(artist).chain(album).max()?;
                Some((best, sv))
            })
            .collect();
        scored.sort_by_key(|&(s, _)| std::cmp::Reverse(s));
        scored.into_iter().map(|(_, sv)| sv).collect()
    }

    /// 把当前 `search_q` 同步给内部 matcher。空 query 也会被推下去,使 matcher 失活。
    /// 同 query 重复调用是无开销 no-op(matcher 内部判等)。
    pub fn sync_query(&self) {
        self.matcher.borrow_mut().set_query(&self.search_q);
    }

    /// 对单段文本跑一次匹配,返回 score + 已映射回原文 char 下标的 `hits`。
    ///
    /// 空 query / 不命中都返回 `None`。每帧渲染时按需调用(已带 MatchableText 缓存
    /// + matcher buffer 复用,开销可忽略)。
    pub fn match_for(&self, text: &str) -> Option<Match> {
        if self.search_q.is_empty() {
            return None;
        }
        self.sync_query();
        let mt = self.matchable_for(text);
        self.matcher.borrow_mut().score(&mt)
    }

    /// 拿 / 构造 一份预处理过的 `MatchableText`。首次见到的文本会算一次拼音。
    fn matchable_for(&self, text: &str) -> Arc<MatchableText> {
        if let Some(mt) = self.matchable_cache.borrow().get(text) {
            return Arc::clone(mt);
        }
        let mt = MatchableText::new(text);
        self.matchable_cache
            .borrow_mut()
            .insert(text.to_owned(), Arc::clone(&mt));
        mt
    }
}

/// 把配置的频谱段映射成 DSP 参数([`mineral_spectrum::SpectrumParams`])。
/// mineral-spectrum 是叶子 crate 不依赖配置,在此(消费侧)做一次显式映射。
///
/// # Params:
///   - `cfg`: 频谱段配置
///
/// # Return:
///   DSP 参数。
fn spectrum_params(cfg: &mineral_config::SpectrumConfig) -> mineral_spectrum::SpectrumParams {
    mineral_spectrum::SpectrumParams::builder()
        .fft_size(*cfg.fft_size())
        .f_min(*cfg.f_min())
        .f_max(*cfg.f_max())
        .log_axis_blend(*cfg.log_axis_blend())
        .db_floor(*cfg.db_floor())
        .db_ceil(*cfg.db_ceil())
        .peak_mix(*cfg.peak_mix())
        .build()
}

#[cfg(test)]
mod tests {
    use mineral_model::SourceKind;

    use crate::test_support::{endserenading, playlist_view};

    use super::AppState;

    /// `queue_current_index` 命中在播歌下标;无在播曲返回 `None`。
    #[test]
    fn queue_current_index_finds_playing() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        let queue = endserenading(5);
        s.playback.track = queue.get(2).cloned();
        s.queue = queue;
        assert_eq!(s.queue_current_index(), Some(2));

        s.playback.track = None;
        assert_eq!(s.queue_current_index(), None);
        Ok(())
    }

    /// 首字母 query `cry` 只命中「春日影」,其它歌单淘汰。
    #[test]
    fn filtered_playlists_initials_pinyin() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.playlists = vec![
            playlist_view("a", "MyGO!!!!!", SourceKind::NETEASE, 1),
            playlist_view("b", "Ave Mujica", SourceKind::NETEASE, 1),
            playlist_view("c", "春日影", SourceKind::NETEASE, 1),
        ];
        s.search_q = "cry".to_owned();
        let names: Vec<&str> = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.as_str())
            .collect();
        assert_eq!(names, vec!["春日影"]);
        Ok(())
    }

    /// 全拼 query `chunying` 命中「春日影」(子序列覆盖 chun + ying)。
    #[test]
    fn filtered_playlists_full_pinyin() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.playlists = vec![
            playlist_view("a", "春日影", SourceKind::NETEASE, 1),
            playlist_view("b", "MyGO!!!!!", SourceKind::NETEASE, 1),
        ];
        s.search_q = "chunying".to_owned();
        let names: Vec<&str> = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.as_str())
            .collect();
        assert_eq!(names, vec!["春日影"]);
        Ok(())
    }

    /// ASCII fuzzy:`my` 命中含 m+y 子序列的项,连续命中(MyGO)排在散开(Ave Mujica)前。
    #[test]
    fn filtered_playlists_consecutive_ranks_first() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.playlists = vec![
            playlist_view("a", "Ave Mujica", SourceKind::NETEASE, 1),
            playlist_view("b", "MyGO!!!!!", SourceKind::NETEASE, 1),
        ];
        s.search_q = "my".to_owned();
        let names: Vec<&str> = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.as_str())
            .collect();
        assert_eq!(names.first().copied(), Some("MyGO!!!!!"));
        Ok(())
    }

    /// `match_for` 命中拼音/首字母时,hits 反向映射回原文 Han 字符下标。
    #[test]
    fn match_for_returns_original_indices() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.search_q = "cry".to_owned();
        let m = s
            .match_for("春日影")
            .ok_or_else(|| color_eyre::eyre::eyre!("cry 应命中春日影"))?;
        assert_eq!(m.hits.as_slice(), &[0u32, 1, 2]);
        Ok(())
    }

    /// 空 query 时 `match_for` 直接返回 `None`,fast path。
    #[test]
    fn match_for_empty_query_returns_none() -> color_eyre::Result<()> {
        let s = AppState::test_default()?;
        assert!(s.match_for("春日影").is_none());
        Ok(())
    }
}
