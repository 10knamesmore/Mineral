//! server 数据的 client 端镜像/拉取缓存域:歌单、曲目、歌词,以及 ♥/本地完整播放次数装饰数据。
//!
//! 全部由 server 事件(TaskEvent)增量灌入;「key 不存在」一律表示还没拉到 /
//! 拉失败,渲染端按 loading / 缺省处理。

use std::collections::hash_map::Entry;

use mineral_model::{Lyrics, PlaylistId, SongId, SourceKind};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::runtime::view_model::{PlaylistEntryView, PlaylistView};

/// 数据镜像/缓存域([`AppState`](crate::runtime::state::AppState) 的数据域)。
pub struct LibraryData {
    /// 已加载的歌单(跨 channel 合并;按到达顺序 append)。
    pub playlists: Vec<PlaylistView>,

    /// 歌单 id → 曲目;不在 map 里表示还没拉到。
    pub tracks: FxHashMap<PlaylistId, Vec<PlaylistEntryView>>,

    /// `tracks` 内容版本:每次歌单曲目落 cache 自增。深度搜索缓存按此失效;
    /// 纯装饰重建(`redecorate_for_source`)不动文本,不 bump。
    pub tracks_generation: u64,

    /// 已提交过 `PlaylistDetail` 请求的歌单(成败都记)。prefetch 据此去重,
    /// 避免**失败**歌单(`tracks` 永远不会被填)被每帧无限重提交而刷屏。
    /// 对齐 cover 的 `covers.pending`。
    pub tracks_requested: FxHashSet<PlaylistId>,

    /// 歌曲 id → 完整结构化歌词(原文 / 逐字 / 翻译 / 罗马音);不在 map 里表示还没拉到 /
    /// 拉失败。channel 层已清洗,client 直接收整份,渲染时按需取各字段。
    pub lyrics: FxHashMap<SongId, Lyrics>,

    /// 各 channel 当前用户喜欢(♥)的歌曲 ID 集合;装饰 entry Song 用。
    /// 缺 source 时该 source 的歌全部按 `loved=false` 渲染。
    pub liked_ids: FxHashMap<SourceKind, FxHashSet<SongId>>,

    /// Mineral 本地自然播完次数及其 selection-scoped 查询状态。
    pub(crate) local_play_counts: LocalPlayCounts,
}

impl LibraryData {
    /// 构造空数据域(全部缓存为空,等 server 事件增量填充)。
    pub(crate) fn new() -> Self {
        Self {
            playlists: Vec::new(),
            tracks: FxHashMap::default(),
            tracks_generation: 0,
            tracks_requested: FxHashSet::default(),
            lyrics: FxHashMap::default(),
            liked_ids: FxHashMap::default(),
            local_play_counts: LocalPlayCounts::new(),
        }
    }
}

/// 本地完播次数，以及每首歌对应的后台查询状态。
pub(crate) struct LocalPlayCounts {
    /// 歌曲 id → 已缓存值或在飞查询状态；缺 id 表示还没查到或查询失败。
    entries: FxHashMap<SongId, LocalPlayCountEntry>,

    /// 上一 tick 已检查的 selection；变化后才允许重试缺失值。
    selected: Option<SongId>,
}

/// 一首歌在本地完播次数 cache 中的完整状态。
#[derive(Clone, Copy)]
enum LocalPlayCountEntry {
    /// 已提交查询，尚未收到 completion event。
    Loading,

    /// 查询期间发生过 eof，随后到达的旧 response 必须丢弃。
    StaleInFlight,

    /// Mineral 本地已记录的自然播完次数。
    Ready(u32),
}

impl LocalPlayCounts {
    /// 构造空缓存。
    fn new() -> Self {
        Self {
            entries: FxHashMap::default(),
            selected: None,
        }
    }

    /// 读取一首歌已缓存的自然播完次数。
    ///
    /// # Params:
    ///   - `song_id`: 目标歌曲
    ///
    /// # Return:
    ///   已查询到的次数；无值或查询失败时为 `None`
    pub(crate) fn get(&self, song_id: &SongId) -> Option<&u32> {
        match self.entries.get(song_id) {
            Some(LocalPlayCountEntry::Ready(count)) => Some(count),
            Some(LocalPlayCountEntry::Loading | LocalPlayCountEntry::StaleInFlight) | None => None,
        }
    }

    /// 当前是否没有任何已查询到的值。
    ///
    /// # Return:
    ///   没有缓存值时为 `true`
    #[cfg(test)]
    pub(crate) fn has_no_cached_values(&self) -> bool {
        self.entries
            .values()
            .all(|entry| !matches!(entry, LocalPlayCountEntry::Ready(_)))
    }

    /// 进入一个 selection，并判断是否应发起查询。
    ///
    /// 成功值直接命中 cache，不因移动 selection 重查；失败结果只在当前 selection 内
    /// 抑制重试，离开后再进入可重新查询。若该歌曲已有请求在飞行，则不重复提交。
    ///
    /// # Params:
    ///   - `song_id`: 新选中的歌曲
    ///
    /// # Return:
    ///   本次需要提交查询时为 `true`
    pub(crate) fn enter_selection(&mut self, song_id: &SongId) -> bool {
        if self.selected.as_ref() == Some(song_id) {
            return false;
        }
        self.selected = Some(song_id.clone());
        match self.entries.entry(song_id.clone()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(LocalPlayCountEntry::Loading);
                true
            }
        }
    }

    /// 离开可查询的歌曲 selection。
    pub(crate) fn leave_selection(&mut self) {
        self.selected = None;
    }

    /// 收束一首歌的后台查询，把有效结果转成 cache，并过滤过期或意外 response。
    ///
    /// # Params:
    ///   - `song_id`: completion event 关联的歌曲
    ///   - `count`: 查询返回的自然播完次数
    ///
    /// # Return:
    ///   新值已写入 cache 时为 `true`
    pub(crate) fn complete(&mut self, song_id: &SongId, count: Option<u32>) -> bool {
        match self.entries.entry(song_id.clone()) {
            Entry::Vacant(_) => false,
            Entry::Occupied(mut entry) => match *entry.get() {
                LocalPlayCountEntry::Loading => {
                    let Some(count) = count else {
                        entry.remove();
                        return false;
                    };
                    entry.insert(LocalPlayCountEntry::Ready(count));
                    true
                }
                LocalPlayCountEntry::StaleInFlight => {
                    entry.remove();
                    false
                }
                LocalPlayCountEntry::Ready(_) => false,
            },
        }
    }

    /// 记录一次自然播完：已有 cache 原地加一，在飞查询标成过期。
    ///
    /// # Params:
    ///   - `song_id`: 自然播完的歌曲
    ///
    /// # Return:
    ///   已有 cache 被更新时为 `true`；尚无基数时为 `false`
    pub(crate) fn note_eof(&mut self, song_id: &SongId) -> bool {
        let Some(entry) = self.entries.get_mut(song_id) else {
            return false;
        };
        match entry {
            LocalPlayCountEntry::Loading => {
                *entry = LocalPlayCountEntry::StaleInFlight;
                false
            }
            LocalPlayCountEntry::StaleInFlight => false,
            LocalPlayCountEntry::Ready(count) => {
                *count = count.saturating_add(1);
                true
            }
        }
    }

    /// 清空所有值与 selection-scoped 查询状态。
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.selected = None;
    }

    /// 当前是否没有在飞行请求，也没有已记录的 selection。
    ///
    /// # Return:
    ///   查询状态完全空闲时为 `true`
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.selected.is_none()
            && self
                .entries
                .values()
                .all(|entry| matches!(entry, LocalPlayCountEntry::Ready(_)))
    }
}
