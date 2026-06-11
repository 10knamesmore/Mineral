//! 应用全局状态。`library.tracks` 中的「key 不存在」== 还没拉到(渲染 "loading…")。

use std::sync::Arc;
use std::time::Duration;

use mineral_model::{PlaylistId, Song, SourceKind};
use mineral_spectrum::SpectrumComputer;
use mineral_task::TaskEvent;
use rustc_hash::FxHashMap;

use mineral_model::{LyricLine, Lyrics};

use crate::components::layout::spectrum::SpectrumState;
use crate::render::anim::{Toggle, ticks16_from_ms};
use crate::runtime::playback::Playback;
use crate::runtime::view_model::{PlaylistView, SongView};

mod covers;
mod library;
mod lyric_view;
mod nav;
mod player;
mod search;
mod view_switch;

pub use covers::CoverHub;
pub use library::LibraryData;
pub use lyric_view::LyricView;
pub use nav::NavState;
pub use player::PlayerMirror;
pub use search::SearchState;
pub use view_switch::ViewSwitch;

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

mod lyric_glide;

/// 应用顶层状态。
pub struct AppState {
    /// 左栏视图切换:Playlists ↔ Library 两态 + 横向过渡,由 [`ViewSwitch`] 合一。
    /// `current()` 给路由 / 选中语义、`eased_in_out()` 给渲染;`== View::X` 直接可比。
    pub view: ViewSwitch,

    /// 全屏播放态:逻辑开关(供按键路由)+ 进退场形变进度(供渲染),由 [`Toggle`] 合一。
    /// `on()` = 全屏、`eased_in_out()` = 形变位置。
    pub fullscreen: Toggle,

    /// 顶栏失焦变灰:`on()` = 已变灰(终端未聚焦)、`eased_in_out()` = 变灰深度。
    /// 终端聚焦态由 [`Self::focused`] 反读;初始 `off`(聚焦)——mode 1004 只报变化、
    /// 不支持的终端永不发事件,降级方向必须是「恒聚焦」。
    pub dim: Toggle,

    /// server 数据镜像/拉取缓存(歌单 / 曲目 / 歌词 + ♥/播放次数装饰)。
    pub library: LibraryData,

    /// 歌词面板显示态(副歌词档 + 全屏手动滚动脱离态)。
    pub lyric_view: LyricView,

    /// 脚本下发的 session 级旋钮覆盖(`Event::UiOverride` 落地;渲染处
    /// 有覆盖读覆盖、无覆盖读配置)。
    pub ui_overrides: crate::runtime::ui_override::UiOverrides,

    /// 列表浏览态(两个列表的光标 + 视口滚动、跨歌单位置记忆、选中变化时刻)。
    pub nav: NavState,

    /// 搜索状态(查询串 / 输入态 + 模糊匹配基建)。
    pub search: SearchState,

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
            view: ViewSwitch::new(ticks16_from_ms(*anim.sweep_ms(), tick_ms)),
            fullscreen: Toggle::new(ticks16_from_ms(*anim.fullscreen_ms(), tick_ms)),
            dim: Toggle::new(ticks16_from_ms(*anim.focus_fade_ms(), tick_ms)),
            library: LibraryData::new(),
            lyric_view: LyricView::new(),
            ui_overrides: crate::runtime::ui_override::UiOverrides::default(),
            nav: NavState::new(),
            search: SearchState::new(),
            player: PlayerMirror::new(),
            playback: Playback::new(),
            spectrum: SpectrumState::new(cfg.tui().spectrum().clone(), tick_ms),
            fft: SpectrumComputer::new(spectrum_params(cfg.tui().spectrum())),
            covers: CoverHub::new(),
            tasks_snapshot: mineral_task::Snapshot {
                running: 0,
                by_lane: FxHashMap::default(),
                by_kind: FxHashMap::default(),
            },
            cfg,
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

    /// 距上次选中变化是否仍在封面 debounce 防抖窗口内(配置 `tui.cover.debounce_ms`)。
    pub fn is_scrolling(&self) -> bool {
        self.nav.last_sel_change.elapsed()
            < Duration::from_millis(*self.cfg.tui().cover().debounce_ms())
    }

    /// 光标与列表视口上下边缘的最小行距(配置 `behavior.scrolloff`)。
    pub(crate) fn scrolloff(&self) -> usize {
        usize::from(*self.cfg.tui().behavior().scrolloff())
    }

    /// 歌单内光标位置记忆档(配置 `behavior.remember_track_pos`)。
    pub(crate) fn track_memory(&self) -> mineral_config::TrackPosMemory {
        *self.cfg.tui().behavior().remember_track_pos()
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
            .nav
            .pending_track_restore
            .as_ref()
            .is_none_or(|p| &p.playlist != id)
        {
            return;
        }
        let Some(pending) = self.nav.pending_track_restore.take() else {
            return;
        };
        let still_there = self.view == View::Library
            && self
                .selected_playlist()
                .is_some_and(|p| p.data.id == pending.playlist);
        if !still_there || self.nav.sel_track != 0 {
            return;
        }
        let Some(tracks) = self.library.tracks.get(&pending.playlist) else {
            return;
        };
        let sel = pending.pos.resolve(tracks);
        self.nav.sel_track = sel;
        // 与 activate 的即时恢复同语义:按屏上相对行还原视口。
        self.nav
            .scroll_track
            .snap_to(sel.saturating_sub(pending.pos.screen_row));
    }

    /// 列表视口滚动平移的缓动拍数(配置 `animation.list_scroll_ms` 折算)。
    pub(crate) fn list_glide_ticks(&self) -> u16 {
        let anim = self.cfg.tui().animation();
        ticks16_from_ms(*anim.list_scroll_ms(), *anim.frame_tick_ms())
    }

    /// 给定一首歌,根据当前 `library.liked_ids` / 未来其他 user-data 装饰成 SongView。
    /// 这是 user-data 写入 SongView 的**唯一入口**;新增 user-data 字段时只改这里。
    fn decorate(&self, song: Song) -> SongView {
        let loved = self
            .library
            .liked_ids
            .get(&song.source())
            .is_some_and(|s| s.contains(&song.id));
        let plays = self.library.play_counts.get(&song.id).copied();
        SongView {
            data: song,
            loved,
            plays,
        }
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
            TaskEvent::PlaylistsFetched { playlists, .. } => {
                self.library
                    .playlists
                    .extend(playlists.iter().cloned().map(|data| PlaylistView { data }));
                if self.nav.sel_playlist >= self.library.playlists.len() {
                    self.nav.sel_playlist = 0;
                }
            }
            TaskEvent::PlaylistTracksFetched { id, tracks } => {
                let decorated = tracks
                    .iter()
                    .cloned()
                    .map(|data| self.decorate(data))
                    .collect();
                self.library.tracks.insert(id.clone(), decorated);
                self.library.tracks_generation = self.library.tracks_generation.wrapping_add(1);
                self.apply_pending_restore(id);
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
            TaskEvent::PlayUrlReady { .. } | TaskEvent::LyricsReady { .. } => {}
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

    /// 当前曲目是否有任一副歌词(翻译 / 罗马音)可切换。无则歌词面板不显示 `t` 提示。
    pub fn has_extra_lyrics(&self) -> bool {
        self.current_lyrics_set()
            .is_some_and(|l| l.has_translation() || l.has_romanization())
    }

    /// 当前生效的副歌词档(当前歌确有该档数据才算生效;`None` 档 / 该档无数据返回 `None`)。
    pub fn active_lyric_extra(&self) -> Option<LyricExtra> {
        let l = self.current_lyrics_set()?;
        match self.lyric_view.extra {
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
        self.lyric_view.extra = match self.lyric_view.extra {
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
    /// `nav.sel_playlist` 的语义随 [`Self::view`] 切换:
    /// - Playlists 视图:filtered 列表的索引,过滤词作用于 playlist 名,渲染、导航、
    ///   selected_playlist 都对齐 filtered。
    /// - Library 视图:raw 列表的索引(进 Library 时已 remap 锁定为「用户进的那条」),
    ///   此时 search.query 作用于 tracks,跟 playlists 过滤无关。
    pub fn selected_playlist(&self) -> Option<&PlaylistView> {
        match self.view.current() {
            View::Playlists => self
                .filtered_playlists()
                .get(self.nav.sel_playlist)
                .copied(),
            View::Library => self.library.playlists.get(self.nav.sel_playlist),
        }
    }

    /// 当前选中歌单的曲目槽位(`None` = 还没拉到)。
    pub fn current_tracks_slot(&self) -> Option<&Vec<SongView>> {
        self.selected_playlist()
            .and_then(|p| self.library.tracks.get(&p.data.id))
    }

    /// 当前选中歌单的曲目列表(slot 未到位时返回空)。
    pub fn current_tracks(&self) -> Vec<SongView> {
        self.current_tracks_slot().cloned().unwrap_or_default()
    }

    /// 给定歌单的总时长(ms);槽位未到位时返回 0。
    pub fn total_duration_ms_of(&self, id: &PlaylistId) -> u64 {
        self.library
            .tracks
            .get(id)
            .map(|tracks| tracks.iter().map(|sv| sv.data.duration_ms).sum())
            .unwrap_or(0)
    }

    /// 当前在播歌在 queue 中的下标(打开浮层时把光标定位到此)。无在播曲返回 `None`。
    pub fn queue_current_index(&self) -> Option<usize> {
        let id = &self.playback.track.as_ref()?.id;
        self.player.queue.iter().position(|s| &s.id == id)
    }

    /// 当前可见(被 search 过滤)的歌单列表。
    ///
    /// 空 query → 原序;非空 query → fzf 风格模糊匹配(拼音/首字母也算命中),
    /// 按 score 降序排,**stable** 保证同分按原序。
    pub fn filtered_playlists(&self) -> Vec<&PlaylistView> {
        if self.search.query.is_empty() {
            return self.library.playlists.iter().collect();
        }
        self.search.sync_query();
        crate::runtime::deep_search::ensure(self);
        let deep = self.search.deep_cache.borrow();
        let mut scored: Vec<(f64, &PlaylistView)> = self
            .library
            .playlists
            .iter()
            .filter_map(|p| {
                let name = self
                    .search
                    .match_for(&p.data.name)
                    .map(|m| f64::from(m.score));
                let inner = deep.score_of(&p.data.id);
                let best = match (name, inner) {
                    (Some(n), Some(i)) => n.max(i),
                    (Some(n), None) => n,
                    (None, Some(i)) => i,
                    (None, None) => return None,
                };
                Some((best, p))
            })
            .collect();
        // total_cmp 全序 + sort_by 稳定:同分项保持原序。
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().map(|(_, p)| p).collect()
    }

    /// 某歌单的深度命中展示载荷(克隆一份给渲染)。空 query / 无命中返回 `None`。
    ///
    /// 调用前提:本帧已有人调过 [`Self::filtered_playlists`](渲染路径必然满足),
    /// 缓存已就绪;这里不再 ensure,避免渲染端反复触发指纹比较。
    pub fn deep_hit_for(&self, id: &PlaylistId) -> Option<crate::runtime::deep_search::DeepHit> {
        if self.search.query.is_empty() {
            return None;
        }
        self.search.deep_cache.borrow().hit_of(id).cloned()
    }

    /// 当前过滤结果里是否存在任何深度命中。渲染端据此决定 match 列要不要占位——
    /// 全员只命中歌单名时不挤压 name 列宽。调用前提同 [`Self::deep_hit_for`]。
    pub fn has_deep_hits(&self) -> bool {
        !self.search.query.is_empty() && self.search.deep_cache.borrow().has_hits()
    }

    /// 当前可见(被 search 过滤)的曲目列表。
    ///
    /// 命中规则:歌名 / 任一艺人 / 专辑名取最高分作为该曲分数。
    pub fn filtered_tracks(&self) -> Vec<SongView> {
        let tracks = self.current_tracks();
        if self.search.query.is_empty() {
            return tracks;
        }
        self.search.sync_query();
        let mut scored: Vec<(u32, SongView)> = tracks
            .into_iter()
            .filter_map(|sv| {
                let name = self.search.match_for(&sv.data.name).map(|m| m.score);
                let artist = sv
                    .data
                    .artists
                    .iter()
                    .filter_map(|a| self.search.match_for(&a.name).map(|m| m.score))
                    .max();
                let album = sv
                    .data
                    .album
                    .as_ref()
                    .and_then(|a| self.search.match_for(&a.name).map(|m| m.score));
                let best = name.into_iter().chain(artist).chain(album).max()?;
                Some((best, sv))
            })
            .collect();
        scored.sort_by_key(|&(s, _)| std::cmp::Reverse(s));
        scored.into_iter().map(|(_, sv)| sv).collect()
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
        s.player.queue = queue;
        assert_eq!(s.queue_current_index(), Some(2));

        s.playback.track = None;
        assert_eq!(s.queue_current_index(), None);
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
        s.search.query = "cry".to_owned();
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
        s.search.query = "chunying".to_owned();
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
        s.search.query = "my".to_owned();
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
        s.search.query = "cry".to_owned();
        let m = s
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
        assert!(s.search.match_for("春日影").is_none());
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
        s.view.switch_to(View::Library);
        s.nav.sel_playlist = 0; // p1
        s.nav.sel_track = 0;
        let pid = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        let tracks = endserenading(5);
        let anchor = tracks
            .get(2)
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 不足 3 首"))?;
        s.nav.pending_track_restore = Some(PendingRestore {
            playlist: pid.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 2,
                screen_row: 0,
            },
        });

        s.apply(&TaskEvent::PlaylistTracksFetched { id: pid, tracks });
        assert_eq!(s.nav.sel_track, 2, "曲目到达后应补落位到记忆行");
        assert!(s.nav.pending_track_restore.is_none(), "pending 应被消费");
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
        s.view.switch_to(View::Library);
        s.nav.sel_playlist = 0;
        s.nav.sel_track = 1; // 已离开进入时的第 0 行
        let pid = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        let tracks = endserenading(5);
        let anchor = tracks
            .get(3)
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 不足 4 首"))?;
        s.nav.pending_track_restore = Some(PendingRestore {
            playlist: pid.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 3,
                screen_row: 0,
            },
        });

        s.apply(&TaskEvent::PlaylistTracksFetched { id: pid, tracks });
        assert_eq!(s.nav.sel_track, 1, "用户已动光标,不得抢落位");
        assert!(s.nav.pending_track_restore.is_none(), "pending 仍应被消费");
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
        s.view.switch_to(View::Library);
        s.nav.sel_playlist = 0;
        s.nav.sel_track = 0;
        let target = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        let other = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p2");
        let tracks = endserenading(5);
        let anchor = tracks
            .first()
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 为空"))?;
        s.nav.pending_track_restore = Some(PendingRestore {
            playlist: target.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 0,
                screen_row: 0,
            },
        });

        s.apply(&TaskEvent::PlaylistTracksFetched { id: other, tracks });
        assert_eq!(s.nav.sel_track, 0);
        assert!(
            s.nav
                .pending_track_restore
                .as_ref()
                .is_some_and(|p| p.playlist == target),
            "非目标歌单到达不应消费 pending"
        );
        Ok(())
    }
}
