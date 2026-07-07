//! 应用全局状态。`library.tracks` 中的「key 不存在」== 还没拉到(渲染 "loading…")。

use std::sync::Arc;
use std::time::Duration;

use mineral_channel_core::Page;
use mineral_model::{Album, AlbumId, Artist, ArtistId, PlaylistId, SearchKind, Song, SourceKind};
use mineral_spectrum::SpectrumComputer;
use mineral_task::{SearchPayload, TaskEvent};
use ratatui::layout::Rect;
use rustc_hash::FxHashMap;

use mineral_model::{LyricLine, Lyrics};

use crate::components::layout::browse::spectrum::SpectrumState;
use crate::render::anim::{Toggle, ticks16_from_ms};
use crate::runtime::marquee::Marquees;
use crate::runtime::playback::Playback;
use crate::runtime::view_model::{PlaylistView, SongView};

mod browse;
mod channel_search;
mod covers;
mod detail;
mod library;
mod lyric;
mod nav;
mod player;
mod search;
mod search_whitelist;
mod view_switch;

pub(crate) use browse::BrowseModel;
pub use browse::BrowsePage;
pub use channel_search::{PromptSegment, SearchFocus, SearchPage, SearchSession};
pub use covers::CoverHub;
pub use detail::{ArtistSection, DetailData, DetailFetch, DetailFrame, EntityRef};
pub use library::LibraryData;
pub use player::PlayerMirror;
pub use search::SearchState;

/// 左栏当前展示的视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// 歌单列表(默认)。
    Playlists,

    /// 已选歌单内的曲目列表。
    Library,
}

/// 当前活跃的布局层(浮层栈之下)。`handle_key` 路由与 `collect_key_context` 共用
/// [`AppState::active_layer`] 算它——「在哪层」只一处真相,杜绝两处漂移。浮层栈叠在其上,
/// 由调用方各自裁决(路由对所有浮层一视同仁;脚本 ctx 只认 queue 浮层光标)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveLayer {
    /// channel 搜索布局态。
    SearchSession,

    /// 本地 `/` 模糊搜索输入态。
    DeepSearch,

    /// 全屏播放态。
    Fullscreen,

    /// 默认浏览态。
    Browse,
}

/// 当前激活的页(浮层栈之下),供按键路由分流到对应 [`Page`](crate::app) 实现。
///
/// 只两页:`Search` 是模态(独立输入树),其余一律 `Browse`——fullscreen / `/` 过滤都是
/// Browse 同一套导航面上的子模式,不另起页。比 [`ActiveLayer`] 粗一档(后者细分子模式供
/// 渲染 / 上下文裁决)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    /// channel 搜索模态页。
    Search,

    /// 浏览页(含 fullscreen / deep-search 子模式)。
    Browse,
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
pub struct AppState {
    /// Browse 布局层的 view 状态:视图切换 + 全屏子模式 + 列表导航 + 歌词 + `/` 过滤,
    /// 由 [`BrowsePage`] 聚合。与 model 数据(library / player)分离。
    pub browse: BrowsePage,

    /// channel 搜索布局态:与 [`BrowsePage::fullscreen`] 同级的全屏级布局态(两者逻辑 `on` 互斥)。
    /// 含布局开关 + 当前源 + 输入焦点 + 焦点环 + per-源会话。
    pub channel_search: SearchPage,

    /// 顶栏失焦变灰:`on()` = 已变灰(终端未聚焦)、`eased_in_out()` = 变灰深度。
    /// 终端聚焦态由 [`Self::focused`] 反读;初始 `off`(聚焦)——mode 1004 只报变化、
    /// 不支持的终端永不发事件,降级方向必须是「恒聚焦」。
    pub dim: Toggle,

    /// server 数据镜像/拉取缓存(歌单 / 曲目 / 歌词 + ♥/播放次数装饰)。
    pub library: LibraryData,

    /// 脚本下发的 session 级旋钮覆盖(`Event::UiOverride` 落地;渲染处
    /// 有覆盖读覆盖、无覆盖读配置)。
    pub ui_overrides: crate::runtime::ui::overrides::UiOverrides,

    /// server 权威播放态镜像(在播歌 / 队列 / 洗牌备份 / 同步版本号)。
    pub player: PlayerMirror,

    /// 播放状态机。
    pub playback: Playback,

    /// 频谱状态(条高 + 平滑)。
    pub spectrum: SpectrumState,

    /// 频谱 FFT 计算器:吃 PCM 样本,出 64 根条的目标高度。
    pub fft: SpectrumComputer,

    /// 封面管线状态(原图/色板缓存、在飞集合、已编码协议)。
    pub covers: CoverHub,

    /// 后台 server scheduler 当前快照(每 tick 由 App 从 `Client::task_snapshot`
    /// 灌入)。**只含**:server 端 ChannelFetch lane(playlists / tracks /
    /// song-url / lyrics / liked)。封面是 client-local 的 [`CoverFetcher`],
    /// 不在这里——见 [`CoverHub::loading`]。
    /// `by_kind` 给 top_status 显示「pl:N tr:N ...」按 kind 拆分用。
    pub tasks_snapshot: mineral_task::Snapshot,

    /// 已加载的全局配置(`Arc` 共享只读):渲染 / 运行时模块经此读各段旋钮
    /// (lyrics 行距、layout 阈值、prefetch 半径、animation 时长等)。
    pub cfg: Arc<mineral_config::Config>,

    /// 上一帧的主帧面积(渲染端每帧回写,`Cell` 因渲染只持 `&AppState`)。
    /// 按键路径据此重算布局求锚点(如弹菜单贴选中行);首帧前为零矩形,
    /// 消费方需容忍空值(placement 的 clamp 兜底)。
    pub frame_area: std::cell::Cell<Rect>,

    /// 各源能力声明镜像(启动时从 server 拉一次)。UI 据此决定渲染哪些入口
    /// (搜索类型 / 歌单写操作键 / 网页链接复制项);缺项 = 该源未注册,入口不画。
    pub caps: FxHashMap<SourceKind, mineral_channel_core::ChannelCaps>,

    /// 溢出标题滚动(marquee)的槽相位状态(节奏来自配置 `animation.marquee_*`)。
    pub(crate) marquees: Marquees,

    /// not playing 待机唱片纹的旋转状态(相位每 tick 推进,节奏来自配置
    /// `animation.vinyl_rev_ms`)。
    pub(crate) vinyl: crate::components::layout::shared::vinyl::VinylSpin,
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
            browse: BrowsePage::new(anim),
            channel_search: SearchPage::new(
                ticks16_from_ms(*anim.fullscreen_ms(), tick_ms),
                ticks16_from_ms(*anim.search_focus_morph_ms(), tick_ms),
            )
            .with_whitelist(search_whitelist::SearchWhitelist::from(
                cfg.tui().search().channel(),
            )),
            dim: Toggle::new(ticks16_from_ms(*anim.focus_fade_ms(), tick_ms)),
            library: LibraryData::new(),
            ui_overrides: crate::runtime::ui::overrides::UiOverrides::default(),
            player: PlayerMirror::new(),
            playback: Playback::new(),
            spectrum: SpectrumState::new(cfg.tui().spectrum().clone(), tick_ms),
            fft: SpectrumComputer::new(spectrum_params(cfg.tui().spectrum())),
            covers: CoverHub::new(
                *cfg.cache().cover_memory(),
                *cfg.cache().cover_protocol_memory(),
            ),
            tasks_snapshot: mineral_task::Snapshot {
                running: 0,
                by_lane: FxHashMap::default(),
                by_kind: FxHashMap::default(),
            },
            marquees: Marquees::from_config(anim.marquee(), tick_ms),
            vinyl: crate::components::layout::shared::vinyl::VinylSpin::from_config(
                *anim.vinyl_rev_ms(),
                tick_ms,
            ),
            cfg,
            frame_area: std::cell::Cell::new(Rect::default()),
            caps: FxHashMap::default(),
        }
    }

    /// 测试构造:defaults 配置(= 接线前硬编码常量)的空状态。
    #[cfg(test)]
    pub(crate) fn test_default() -> color_eyre::Result<Self> {
        Ok(Self::new(Arc::new(mineral_config::Config::defaults()?)))
    }

    /// 终端是否持有输入焦点。从 [`Self::dim`] 反读(变灰 = 未聚焦);上报 daemon 用。
    pub fn focused(&self) -> bool {
        !self.dim.on()
    }

    /// 推进一帧的各动画 / 相位状态(主循环每 tick 恰调一次):视图切换扫入、全屏形变、
    /// 搜索布局、marquee 相位、失焦渐变、歌词滚动。
    pub fn tick_frame(&mut self) {
        self.browse.view.tick();
        self.browse.fullscreen.tick();
        self.channel_search.tick();
        self.marquees.tick();
        self.vinyl.tick();
        self.dim.tick();
        self.tick_lyric_scroll();
    }

    /// 是否处于文本输入态:本地 `/` 模糊 typing,或 channel-search 搜索框(prompt 焦点)。
    /// 全局逃生口 / 单键快捷在此让位字符输入——输入态的按键是文本,不是命令。
    pub(crate) fn in_text_input(&self) -> bool {
        self.browse.search.typing
            || (self.channel_search.active.on() && self.channel_search.focus == SearchFocus::Prompt)
    }

    /// 当前激活的页(见 [`PageKind`]):Search 模态优先,否则归 Browse。供 `handle_key` 顶层
    /// 路由分流到对应 Page 实现;子模式细分仍读 [`Self::active_layer`]。
    pub(crate) fn page_kind(&self) -> PageKind {
        if self.channel_search.active.on() {
            PageKind::Search
        } else {
            PageKind::Browse
        }
    }

    /// 当前活跃的布局层(见 [`ActiveLayer`])。浮层栈在其之上,由调用方单独裁决。
    pub(crate) fn active_layer(&self) -> ActiveLayer {
        if self.channel_search.active.on() {
            ActiveLayer::SearchSession
        } else if self.browse.search.typing {
            ActiveLayer::DeepSearch
        } else if self.browse.fullscreen.on() {
            ActiveLayer::Fullscreen
        } else {
            ActiveLayer::Browse
        }
    }

    /// 距上次选中变化是否仍在封面 debounce 防抖窗口内(配置 `tui.cover.debounce_ms`)。
    pub fn is_scrolling(&self) -> bool {
        self.browse.nav.last_sel_change.elapsed()
            < Duration::from_millis(*self.cfg.tui().cover().debounce_ms())
    }

    /// 光标与列表视口上下边缘的最小行距(配置 `behavior.scrolloff`)。
    pub(crate) fn scrolloff(&self) -> usize {
        usize::from(*self.cfg.tui().behavior().scrolloff())
    }

    /// 曲目到达时兑现挂起的位置恢复(进歌单时曲目还没拉到的延迟落位)。
    ///
    /// 仅当用户仍停在该歌单的 Library 视图、且光标未被动过(还在进入时的第 0 行)
    /// 才落位——不抢用户操作;无论是否落位,匹配歌单的 pending 都就此消费。
    ///
    /// # Params:
    ///   - `id`: 刚落 cache 的歌单
    fn apply_pending_restore(&mut self, id: &PlaylistId) {
        if self
            .browse
            .nav
            .pending_track_restore
            .as_ref()
            .is_none_or(|p| &p.playlist != id)
        {
            return;
        }
        let Some(pending) = self.browse.nav.pending_track_restore.take() else {
            return;
        };
        let still_there = self.browse.view == View::Library
            && self
                .selected_playlist()
                .is_some_and(|p| p.data.id == pending.playlist);
        if !still_there || self.browse.nav.track.sel() != 0 {
            return;
        }
        let Some(tracks) = self.library.tracks.get(&pending.playlist) else {
            return;
        };
        let sel = pending.pos.resolve(tracks);
        // 与 activate 的即时恢复同语义:光标落位 + 按屏上相对行瞬时还原视口。
        self.browse.nav.track.place(sel, pending.pos.screen_row);
    }

    /// 列表视口滚动平移的缓动拍数(配置 `animation.list_scroll_ms` 折算)。
    pub(crate) fn list_glide_ticks(&self) -> u16 {
        let anim = self.cfg.tui().animation();
        ticks16_from_ms(*anim.list_scroll_ms(), *anim.frame_tick_ms())
    }

    /// 给定一首歌,根据当前 `library.liked_ids` / 未来其他 user-data 装饰成 SongView。
    /// 这是 user-data 写入 SongView 的**唯一入口**;新增 user-data 字段时只改这里。
    fn decorate(&self, song: Song) -> SongView {
        let loved = self.is_liked(&song);
        let plays = self.library.play_counts.get(&song.id).copied();
        SongView {
            data: song,
            loved,
            plays,
        }
    }

    /// 一首歌是否已收藏（查 `library.liked_ids` 该源的桶）。
    pub(crate) fn is_liked(&self, song: &Song) -> bool {
        self.library
            .liked_ids
            .get(&song.source())
            .is_some_and(|s| s.contains(&song.id))
    }

    /// 本地乐观切换一首歌的喜欢态(翻转 `library.liked_ids` 并重装该源曲目)。
    ///
    /// 不等 server 确认——按键即时反馈;真实持久化由 `client.toggle_love` 触发,
    /// 失败由下次 `LikedSongIdsFetched` fetch 纠正。
    ///
    /// # Params:
    ///   - `song`: 目标歌曲
    pub fn toggle_loved_local(&mut self, song: &Song) {
        let set = self.library.liked_ids.entry(song.source()).or_default();
        if !set.remove(&song.id) {
            set.insert(song.id.clone());
        }
        self.redecorate_for_source(song.source());
    }

    /// 某个 channel 的 user-data 到位 / 变化时,把 `library.tracks` 里属于该 source
    /// 的 SongView 全部按当前 `decorate` 重建一遍。
    /// 跨 source 的歌单不动(decoration data 是 per-source 的)。
    fn redecorate_for_source(&mut self, source: SourceKind) {
        let cache = std::mem::take(&mut self.library.tracks);
        self.library.tracks = cache
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
            TaskEvent::LibrarySnapshot { playlists } => {
                // 合并快照整表替换:跨源顺序由 server 唯一权威(curate 出口
                // 变换后),client 不再自行按源拼接。
                self.library.playlists = playlists
                    .iter()
                    .cloned()
                    .map(|data| PlaylistView { data })
                    .collect();
                if self.browse.nav.playlist.sel() >= self.library.playlists.len() {
                    self.browse.nav.playlist.set_sel(0);
                }
            }
            // server 已聚合进 LibrarySnapshot,理论不会到 client。defensive:跳过。
            TaskEvent::PlaylistsFetched { .. } => {}
            TaskEvent::PlaylistDetailFetched { id, playlist } => {
                // 歌单详情含元信息 + 曲目;library 与 detail 都只取曲目(歌单元信息走
                // sidebar 列表那份 / detail 帧的 entity 占位)。
                let decorated = playlist
                    .songs
                    .iter()
                    .cloned()
                    .map(|data| self.decorate(data))
                    .collect();
                self.library.tracks.insert(id.clone(), decorated);
                self.library.tracks_generation = self.library.tracks_generation.wrapping_add(1);
                self.apply_pending_restore(id);
                // detail 歌单帧也吃这批曲目(若当前栈顶正等它)。
                if let Some(kr) = self.channel_search.active_results_mut() {
                    kr.fill_playlist_tracks(id, playlist.songs.clone());
                }
            }
            TaskEvent::LikedSongIdsFetched { source, ids } => {
                self.library.liked_ids.insert(*source, ids.clone());
                self.redecorate_for_source(*source);
            }
            TaskEvent::RemotePlayCountFetched { song_id, count } => {
                self.library.play_counts.insert(song_id.clone(), *count);
                self.redecorate_for_source(song_id.namespace());
            }
            // server 已 filter,理论不会到 client。defensive:跳过。
            TaskEvent::PlayUrlReady { .. }
            | TaskEvent::SongUrlFailed { .. }
            | TaskEvent::LyricsReady { .. } => {}
            TaskEvent::SearchResults {
                source,
                kind,
                query,
                page,
                payload,
                has_more,
            } => self.apply_search_results(*source, *kind, query, *page, payload, *has_more),
            TaskEvent::ArtistDetailFetched { id, artist } => self.apply_artist_detail(id, artist),
            TaskEvent::ArtistAlbumsFetched { id, albums, .. } => {
                self.apply_artist_albums(id, albums);
            }
            TaskEvent::AlbumDetailFetched { id, album } => self.apply_album_detail(id, album),
            // 歌单写操作完结由后续里程碑(歌单管理)消费;先吞掉保持 match 穷尽。
            TaskEvent::PlaylistWriteDone { .. } => {}
        }
    }

    /// 把一页搜索结果落进配对的会话：按事件自带 `source` 找会话、`query` 配对（源级，
    /// 改 query 作废全部 kind 桶），事件自带 `kind` 决定存哪个桶——这样切 kind 时旧 kind
    /// 的飞行响应也能落对桶，per-kind 缓存才完整。首页（`offset == 0`）建桶、翻页 append。
    ///
    /// # Params:
    ///   - `source` / `kind` / `query`: 回带的请求三元组（source 找会话、query 配对、kind 选桶）
    ///   - `page`: 分页参数（`offset == 0` 首页建桶，否则 append）
    ///   - `payload`: 结果载荷
    ///   - `has_more`: 源的显式翻页信号
    fn apply_search_results(
        &mut self,
        source: SourceKind,
        kind: SearchKind,
        query: &str,
        page: Page,
        payload: &SearchPayload,
        has_more: Option<bool>,
    ) {
        // caps 先读:随后 session 借走 self.channel_search。歌手源的可用分区落定桶级判定,让歌手
        // root 帧把分区收到首个可用区(无热门曲的源如 B站即只有 Albums;含后续 set_sel 复位)。
        let sections = self
            .caps
            .get(&source)
            .map(|channel_caps| channel_caps.artist_sections().clone());
        let Some(session) = self.channel_search.session_for_mut(source) else {
            return;
        };
        if session.query() != query {
            return;
        }
        session.apply_page(kind, payload.clone(), page, has_more);
        if let Some(sections) = sections {
            session.apply_sections(kind, sections);
        }
    }

    /// ArtistDetail 回包：落到当前 detail 栈顶帧（若正等这个歌手；否则丢弃）。
    fn apply_artist_detail(&mut self, id: &ArtistId, artist: &Artist) {
        if let Some(kr) = self.channel_search.active_results_mut() {
            kr.fill_artist_detail(id, Box::new(artist.clone()));
        }
    }

    /// ArtistAlbums 回包：落到当前 detail 栈顶帧（若正等这个歌手）。
    fn apply_artist_albums(&mut self, id: &ArtistId, albums: &[Album]) {
        if let Some(kr) = self.channel_search.active_results_mut() {
            kr.fill_artist_albums(id, albums.to_vec());
        }
    }

    /// AlbumDetail 回包：完整专辑落到当前 detail 栈顶帧（若正等这张专辑）。
    fn apply_album_detail(&mut self, id: &AlbumId, album: &Album) {
        if let Some(kr) = self.channel_search.active_results_mut() {
            kr.fill_album_detail(id, Box::new(album.clone()));
        }
    }

    /// 当前曲目的完整歌词集合;未拉到时返回 `None`。
    fn current_lyrics_set(&self) -> Option<&Lyrics> {
        let song = self.playback.track.as_ref()?;
        self.library.lyrics.get(&song.id)
    }

    /// 当前曲目的歌词行序列(行级 / 逐字 / 有时间 / 无时间混排,翻译 / 罗马音已内嵌在
    /// 各行上);未拉到时返回 `None`。
    pub fn current_lines(&self) -> Option<&[LyricLine]> {
        self.current_lyrics_set().map(|l| l.lines.as_slice())
    }

    /// 当前正在唱的歌词行文本,供窗口标题用。时间轴失真档(顶换流时长对不上)返回
    /// `None`——与歌词面板同口径,不显示错行;无同步 / 无当前行同样 `None`。
    /// 逐字行文本按需拼接故返回拥有串。
    pub(crate) fn active_title_lyric(&self) -> Option<String> {
        if self.playback.sync_trust() == crate::runtime::playback::SyncTrust::Broken {
            return None;
        }
        let lines = self.current_lines()?;
        let idx = mineral_model::current_line(lines, self.playback.position_ms)?;
        lines.get(idx).map(|line| line.kind.text().into_owned())
    }

    /// 当前曲目是否有任一副歌词(翻译 / 罗马音)可切换。无则歌词面板不显示 `t` 提示。
    pub fn has_extra_lyrics(&self) -> bool {
        self.current_lyrics_set()
            .is_some_and(|l| l.has_translation() || l.has_romanization())
    }

    /// 当前生效的副歌词档(当前歌确有该档数据才算生效;`None` 档 / 该档无数据返回 `None`)。
    pub fn active_lyric_extra(&self) -> Option<LyricExtra> {
        let l = self.current_lyrics_set()?;
        match self.browse.lyric_view.extra {
            LyricExtra::None => None,
            LyricExtra::Translation if l.has_translation() => Some(LyricExtra::Translation),
            LyricExtra::Romanization if l.has_romanization() => Some(LyricExtra::Romanization),
            LyricExtra::Translation | LyricExtra::Romanization => None,
        }
    }

    /// 循环副歌词档:`None → Translation → Romanization → None`,跳过当前歌为空的档。
    /// 翻译 / 罗马音都缺时停在 `None`。
    pub fn cycle_lyric_extra(&mut self) {
        let has_trans = self
            .current_lyrics_set()
            .is_some_and(Lyrics::has_translation);
        let has_roma = self
            .current_lyrics_set()
            .is_some_and(Lyrics::has_romanization);
        self.browse.lyric_view.extra = match self.browse.lyric_view.extra {
            LyricExtra::None if has_trans => LyricExtra::Translation,
            LyricExtra::None if has_roma => LyricExtra::Romanization,
            LyricExtra::None => LyricExtra::None,
            LyricExtra::Translation if has_roma => LyricExtra::Romanization,
            LyricExtra::Translation => LyricExtra::None,
            LyricExtra::Romanization => LyricExtra::None,
        };
    }

    /// 构造 Browse 视图逻辑所需的只读模型借用(library + cfg);供下方 forwarder 调 BrowsePage。
    fn browse_model(&self) -> browse::BrowseModel<'_> {
        browse::BrowseModel {
            library: &self.library,
            cfg: &self.cfg,
        }
    }

    /// 返回当前选中歌单的引用。
    ///
    /// `nav.sel_playlist` 的语义随 [`BrowsePage::view`] 切换:
    /// - Playlists 视图:filtered 列表的索引,过滤词作用于 playlist 名,渲染、导航、
    ///   selected_playlist 都对齐 filtered。
    /// - Library 视图:raw 列表的索引(进 Library 时已 remap 锁定为「用户进的那条」),
    ///   此时 search.query 作用于 tracks,跟 playlists 过滤无关。
    pub fn selected_playlist(&self) -> Option<&PlaylistView> {
        self.browse.selected_playlist(self.browse_model())
    }

    /// 当前选中歌单的曲目槽位(`None` = 还没拉到)。
    pub fn current_tracks_slot(&self) -> Option<&Vec<SongView>> {
        self.browse.current_tracks_slot(self.browse_model())
    }

    /// 给定歌单的总时长(ms);槽位未到位时返回 0。未知时长的曲目不计入(只反映已知部分)。
    pub fn total_duration_ms_of(&self, id: &PlaylistId) -> u64 {
        self.library
            .tracks
            .get(id)
            .map(|tracks| tracks.iter().filter_map(|sv| sv.data.duration_ms).sum())
            .unwrap_or(0)
    }

    /// 当前在播歌在 queue 中的下标(打开浮层定位光标 / prefetch 邻居 / 封面预热都用它)。
    /// 无在播曲返回 `None`。
    ///
    /// 优先信任 server 的在播锚点 `queue_sel`——队列含重复曲时,这是唯一能精确指出
    /// 在播是哪一行的依据(按歌曲身份 first-match 会错指到首个副本)。仅当锚点确实指向
    /// 在播歌时采纳;否则(在播歌不在队列 / 锚点陈旧)退回身份 first-match,保住
    /// 「在播曲不在队列」时返回 `None` 的既有语义。
    pub fn queue_current_index(&self) -> Option<usize> {
        let id = &self.playback.track.as_ref()?.id;
        let sel = self.player.queue_sel;
        if self.player.queue.get(sel).is_some_and(|s| &s.id == id) {
            return Some(sel);
        }
        self.player.queue.iter().position(|s| &s.id == id)
    }

    /// 当前可见(被 search 过滤)的歌单列表。
    ///
    /// 空 query → 原序;非空 query → fzf 风格模糊匹配(拼音/首字母也算命中),
    /// 按 score 降序排,**stable** 保证同分按原序。
    pub fn filtered_playlists(&self) -> Vec<&PlaylistView> {
        self.browse.filtered_playlists(self.browse_model())
    }

    /// 某歌单的深度命中展示载荷(克隆一份给渲染)。空 query / 无命中返回 `None`。
    ///
    /// 调用前提:本帧已有人调过 [`Self::filtered_playlists`](渲染路径必然满足),
    /// 缓存已就绪;这里不再 ensure,避免渲染端反复触发指纹比较。
    pub fn deep_hit_for(&self, id: &PlaylistId) -> Option<crate::runtime::deep_search::DeepHit> {
        self.browse.deep_hit_for(id)
    }

    /// 当前过滤结果里是否存在任何深度命中。渲染端据此决定 match 列要不要占位——
    /// 全员只命中歌单名时不挤压 name 列宽。调用前提同 [`Self::deep_hit_for`]。
    pub fn has_deep_hits(&self) -> bool {
        self.browse.has_deep_hits()
    }

    /// 当前可见(被 search 过滤)的曲目列表。
    ///
    /// 命中规则:歌名 / 别名 / 任一艺人 / 专辑名取最高分作为该曲分数。
    pub fn filtered_tracks(&self) -> Vec<SongView> {
        self.browse.filtered_tracks(self.browse_model())
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
    use mineral_model::{SearchKind, SourceKind};

    use crate::test_support::{endserenading, playlist_view};

    use super::AppState;

    /// `queue_current_index` 命中在播歌下标;无在播曲返回 `None`。
    #[test]
    fn queue_current_index_finds_playing() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        let queue = endserenading(5);
        s.playback.track = queue.get(2).cloned();
        s.player.queue = queue;
        assert_eq!(s.queue_current_index(), Some(2));

        s.playback.track = None;
        assert_eq!(s.queue_current_index(), None);
        Ok(())
    }

    /// 重复曲:`queue_current_index` 采纳 server 锚点 `queue_sel`,精确指向在播的那个
    /// 副本,而非按身份回退到首个副本。
    #[test]
    fn queue_current_index_prefers_anchor_over_identity() -> color_eyre::Result<()> {
        use mineral_test::song;
        let mut s = AppState::test_default()?;
        s.player.queue = vec![song("a"), song("b"), song("a"), song("b")];
        s.playback.track = Some(song("a"));
        s.player.queue_sel = 2; // 第二个 a 正在播
        assert_eq!(s.queue_current_index(), Some(2), "应采纳锚点,而非首个 a@0");

        // 锚点不指向在播歌(在播曲不在队列)→ 退回身份匹配,找不到返回 None。
        s.playback.track = Some(song("z"));
        s.player.queue_sel = 2;
        assert_eq!(s.queue_current_index(), None, "在播曲不在队列时仍返回 None");
        Ok(())
    }

    /// 合并快照整表替换:重复到达不追加不挪尾,顺序由 server 权威;
    /// 选中超界夹回 0。
    #[test]
    fn library_snapshot_replaces_whole_table() -> color_eyre::Result<()> {
        use mineral_model::{Playlist, PlaylistId};
        use mineral_task::TaskEvent;
        let pl = |id: &str, name: &str| {
            Playlist::builder()
                .id(PlaylistId::new(SourceKind::NETEASE, id))
                .name(name.to_owned())
                .build()
        };
        let mut s = AppState::test_default()?;
        s.apply(&TaskEvent::LibrarySnapshot {
            playlists: vec![pl("p1", "甲"), pl("p2", "乙")],
        });
        let names = |s: &AppState| {
            s.library
                .playlists
                .iter()
                .map(|p| p.data.name.clone())
                .collect::<Vec<String>>()
        };
        assert_eq!(names(&s), vec!["甲", "乙"]);
        s.browse.nav.playlist.set_sel(1);
        // 新快照(重排 + 藏掉一个)整表替换:无重复、无挪尾,超界选中夹回 0。
        s.apply(&TaskEvent::LibrarySnapshot {
            playlists: vec![pl("p2", "乙")],
        });
        assert_eq!(names(&s), vec!["乙"], "整表替换,不残留旧条目");
        assert_eq!(s.browse.nav.playlist.sel(), 0, "选中超界夹回");
        Ok(())
    }

    /// 首字母 query `cry` 只命中「春日影」,其它歌单淘汰。
    #[test]
    fn filtered_playlists_initials_pinyin() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.library.playlists = vec![
            playlist_view("a", "MyGO!!!!!", SourceKind::NETEASE, 1),
            playlist_view("b", "Ave Mujica", SourceKind::NETEASE, 1),
            playlist_view("c", "春日影", SourceKind::NETEASE, 1),
        ];
        s.browse.search.set_query("cry");
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
        s.library.playlists = vec![
            playlist_view("a", "春日影", SourceKind::NETEASE, 1),
            playlist_view("b", "MyGO!!!!!", SourceKind::NETEASE, 1),
        ];
        s.browse.search.set_query("chunying");
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
        s.library.playlists = vec![
            playlist_view("a", "Ave Mujica", SourceKind::NETEASE, 1),
            playlist_view("b", "MyGO!!!!!", SourceKind::NETEASE, 1),
        ];
        s.browse.search.set_query("my");
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
        s.browse.search.set_query("cry");
        let m = s
            .browse
            .search
            .match_for("春日影")
            .ok_or_else(|| color_eyre::eyre::eyre!("cry 应命中春日影"))?;
        assert_eq!(m.hits.as_slice(), &[0u32, 1, 2]);
        Ok(())
    }

    /// 空 query 时 `match_for` 直接返回 `None`,fast path。
    #[test]
    fn match_for_empty_query_returns_none() -> color_eyre::Result<()> {
        let s = AppState::test_default()?;
        assert!(s.browse.search.match_for("春日影").is_none());
        Ok(())
    }

    /// 挂着 pending 时曲目到达:用户仍停在该歌单且光标未动 → 按双锚补落位,
    /// pending 消费掉。
    #[test]
    fn pending_restore_lands_when_tracks_arrive() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;
        use mineral_task::TaskEvent;

        use crate::runtime::state::View;
        use crate::runtime::track_pos::{PendingRestore, TrackPos};
        use crate::test_support::{endserenading, state_with_playlists};

        let mut s = state_with_playlists()?;
        s.browse.view.switch_to(View::Library);
        s.browse.nav.playlist.set_sel(0); // p1
        s.browse.nav.track.set_sel(0);
        let pid = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        let tracks = endserenading(5);
        let anchor = tracks
            .get(2)
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 不足 3 首"))?;
        s.browse.nav.pending_track_restore = Some(PendingRestore {
            playlist: pid.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 2,
                screen_row: 0,
            },
        });

        let playlist = Box::new(
            mineral_model::Playlist::builder()
                .id(pid.clone())
                .name(String::new())
                .songs(tracks)
                .build(),
        );
        s.apply(&TaskEvent::PlaylistDetailFetched { id: pid, playlist });
        assert_eq!(s.browse.nav.track.sel(), 2, "曲目到达后应补落位到记忆行");
        assert!(
            s.browse.nav.pending_track_restore.is_none(),
            "pending 应被消费"
        );
        Ok(())
    }

    /// 用户在曲目到达前已自己动过光标:不抢操作,pending 静默作废。
    #[test]
    fn pending_restore_yields_to_user_movement() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;
        use mineral_task::TaskEvent;

        use crate::runtime::state::View;
        use crate::runtime::track_pos::{PendingRestore, TrackPos};
        use crate::test_support::{endserenading, state_with_playlists};

        let mut s = state_with_playlists()?;
        s.browse.view.switch_to(View::Library);
        s.browse.nav.playlist.set_sel(0);
        s.browse.nav.track.set_sel(1); // 已离开进入时的第 0 行
        let pid = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        let tracks = endserenading(5);
        let anchor = tracks
            .get(3)
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 不足 4 首"))?;
        s.browse.nav.pending_track_restore = Some(PendingRestore {
            playlist: pid.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 3,
                screen_row: 0,
            },
        });

        let playlist = Box::new(
            mineral_model::Playlist::builder()
                .id(pid.clone())
                .name(String::new())
                .songs(tracks)
                .build(),
        );
        s.apply(&TaskEvent::PlaylistDetailFetched { id: pid, playlist });
        assert_eq!(s.browse.nav.track.sel(), 1, "用户已动光标,不得抢落位");
        assert!(
            s.browse.nav.pending_track_restore.is_none(),
            "pending 仍应被消费"
        );
        Ok(())
    }

    /// 别的歌单先到:pending 不消费、不落位,继续等目标歌单。
    #[test]
    fn pending_restore_ignores_other_playlists() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;
        use mineral_task::TaskEvent;

        use crate::runtime::state::View;
        use crate::runtime::track_pos::{PendingRestore, TrackPos};
        use crate::test_support::{endserenading, state_with_playlists};

        let mut s = state_with_playlists()?;
        s.browse.view.switch_to(View::Library);
        s.browse.nav.playlist.set_sel(0);
        s.browse.nav.track.set_sel(0);
        let target = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        let other = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p2");
        let tracks = endserenading(5);
        let anchor = tracks
            .first()
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 为空"))?;
        s.browse.nav.pending_track_restore = Some(PendingRestore {
            playlist: target.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 0,
                screen_row: 0,
            },
        });

        let playlist = Box::new(
            mineral_model::Playlist::builder()
                .id(other.clone())
                .name(String::new())
                .songs(tracks)
                .build(),
        );
        s.apply(&TaskEvent::PlaylistDetailFetched {
            id: other,
            playlist,
        });
        assert_eq!(s.browse.nav.track.sel(), 0);
        assert!(
            s.browse
                .nav
                .pending_track_restore
                .as_ref()
                .is_some_and(|p| p.playlist == target),
            "非目标歌单到达不应消费 pending"
        );
        Ok(())
    }

    /// 造一个已入会(源 NETEASE、kind Song、query 给定)的 AppState,供 SearchResults 配对测试用。
    fn state_in_search(query: &str) -> color_eyre::Result<AppState> {
        use mineral_channel_core::ChannelCaps;
        use mineral_model::{SearchKind, SourceKind};
        use rustc_hash::FxHashMap;

        let mut s = AppState::test_default()?;
        let mut caps = FxHashMap::default();
        caps.insert(
            SourceKind::NETEASE,
            ChannelCaps::builder()
                .searchable(vec![SearchKind::Song])
                .playlist_edit(false)
                .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                    mineral_channel_core::ArtistSectionKind::TopSongs,
                    mineral_channel_core::ArtistSectionKind::Albums,
                ]))
                .build(),
        );
        s.caps = caps;
        s.channel_search.enter(&s.caps);
        if let Some(session) = s.channel_search.current_mut() {
            session.set_query(query.to_owned());
        }
        Ok(s)
    }

    /// 读当前会话的结果条数(无结果 / 非 Songs 载荷计 0)。
    fn session_song_count(s: &AppState) -> usize {
        use mineral_task::SearchPayload;
        match s.channel_search.active_results().map(|kr| &kr.results) {
            Some(SearchPayload::Songs(songs)) => songs.len(),
            _ => 0,
        }
    }

    /// query 配对的 SearchResults 落进当前会话。
    #[test]
    fn search_results_populate_matching_session() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::test_support::endserenading;

        let mut s = state_in_search("hello")?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "hello".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(2)),
            has_more: None,
        });
        assert_eq!(session_song_count(&s), 2, "配对结果入会");
        Ok(())
    }

    /// query 已变的过期 SearchResults 直接丢弃,不污染当前会话。
    #[test]
    fn stale_search_results_dropped() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::test_support::endserenading;

        let mut s = state_in_search("hello")?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "stale".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(5)),
            has_more: None,
        });
        assert_eq!(session_song_count(&s), 0, "过期响应不入会");
        Ok(())
    }

    /// 造一个入会(源 NETEASE、给定 kind、query)的 AppState。
    fn state_searching(query: &str, kind: SearchKind) -> color_eyre::Result<AppState> {
        use mineral_channel_core::ChannelCaps;
        use rustc_hash::FxHashMap;

        let mut s = AppState::test_default()?;
        let mut caps = FxHashMap::default();
        caps.insert(
            SourceKind::NETEASE,
            ChannelCaps::builder()
                .searchable(vec![kind])
                .playlist_edit(false)
                .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                    mineral_channel_core::ArtistSectionKind::TopSongs,
                    mineral_channel_core::ArtistSectionKind::Albums,
                ]))
                .build(),
        );
        s.caps = caps;
        s.channel_search.enter(&s.caps);
        if let Some(session) = s.channel_search.current_mut() {
            session.set_query(query.to_owned());
        }
        Ok(s)
    }

    /// 造一张专辑(测试 helper)。
    fn album_fixture(raw: &str) -> mineral_model::Album {
        mineral_model::Album::builder()
            .id(mineral_model::AlbumId::new(SourceKind::NETEASE, raw))
            .name(format!("album {raw}"))
            .build()
    }

    /// 造一个歌手(测试 helper)。
    fn artist_fixture(raw: &str) -> mineral_model::Artist {
        mineral_model::Artist::builder()
            .id(mineral_model::ArtistId::new(SourceKind::NETEASE, raw))
            .name(format!("artist {raw}"))
            .build()
    }

    /// AlbumSongs 回包落到「选中专辑」的 detail 栈顶帧(配对成功)。
    #[test]
    fn album_songs_fill_selected_detail_frame() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::AlbumId;
        use mineral_task::{SearchPayload, TaskEvent};

        use super::detail::DetailData;

        let mut s = state_searching("q", SearchKind::Album)?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![album_fixture("al1")]),
            has_more: None,
        });
        // detail root = al1，fetch = AlbumDetail(al1)。喂该专辑完整详情(含曲目)。
        s.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .songs(crate::test_support::endserenading(3))
                    .build(),
            ),
        });
        let kr = s
            .channel_search
            .active_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        let frame = kr
            .detail
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 detail root"))?;
        match &frame.data {
            Some(DetailData::Album(a)) => assert_eq!(a.songs.len(), 3, "专辑详情落帧"),
            _ => color_eyre::eyre::bail!("detail 帧应填 Album"),
        }
        Ok(())
    }

    /// 不匹配的 AlbumSongs(别的专辑 id)不污染当前帧。
    #[test]
    fn mismatched_album_songs_dropped() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::AlbumId;
        use mineral_task::{SearchPayload, TaskEvent};

        let mut s = state_searching("q", SearchKind::Album)?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![album_fixture("al1")]),
            has_more: None,
        });
        s.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "OTHER"),
            album: Box::new(
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "OTHER"))
                    .name("other".to_owned())
                    .songs(crate::test_support::endserenading(3))
                    .build(),
            ),
        });
        let kr = s
            .channel_search
            .active_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        let frame = kr
            .detail
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 detail root"))?;
        assert!(frame.data.is_none(), "别的专辑回包不落当前帧");
        Ok(())
    }

    /// 歌手详情两路(热门曲 + 专辑列表)分别到货、合并进同一帧。
    #[test]
    fn artist_detail_and_albums_merge() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::ArtistId;
        use mineral_task::{SearchPayload, TaskEvent};

        use super::detail::DetailData;

        let mut s = state_searching("q", SearchKind::Artist)?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Artist,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Artists(vec![artist_fixture("ar1")]),
            has_more: None,
        });
        let id = ArtistId::new(SourceKind::NETEASE, "ar1");
        s.apply(&TaskEvent::ArtistDetailFetched {
            id: id.clone(),
            artist: Box::new(artist_fixture("ar1")),
        });
        s.apply(&TaskEvent::ArtistAlbumsFetched {
            id,
            page: Page::default(),
            albums: vec![album_fixture("al1"), album_fixture("al2")],
        });
        let kr = s
            .channel_search
            .active_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        let frame = kr
            .detail
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 detail root"))?;
        match &frame.data {
            Some(DetailData::Artist { detail, albums }) => {
                assert!(detail.is_some(), "热门曲那一路到货");
                assert_eq!(albums.as_ref().map(Vec::len), Some(2), "专辑那一路到货");
            }
            _ => color_eyre::eyre::bail!("detail 帧应是 Artist"),
        }
        Ok(())
    }
}
