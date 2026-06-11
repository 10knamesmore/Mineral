//! server 数据的 client 端镜像/拉取缓存域:歌单、曲目、歌词,以及 ♥/播放次数装饰数据。
//!
//! 全部由 server 事件(TaskEvent)增量灌入;「key 不存在」一律表示还没拉到 /
//! 拉失败,渲染端按 loading / 缺省处理。

use mineral_model::{Lyrics, PlaylistId, SongId, SourceKind};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::runtime::view_model::{PlaylistView, SongView};

/// 数据镜像/缓存域([`AppState`](crate::runtime::state::AppState) 的数据域)。
pub struct LibraryData {
    /// 已加载的歌单(跨 channel 合并;按到达顺序 append)。
    pub playlists: Vec<PlaylistView>,

    /// 歌单 id → 曲目;不在 map 里表示还没拉到。
    pub tracks: FxHashMap<PlaylistId, Vec<SongView>>,

    /// `tracks` 内容版本:每次歌单曲目落 cache 自增。深度搜索缓存按此失效;
    /// 纯装饰重建(`redecorate_for_source`)不动文本,不 bump。
    pub tracks_generation: u64,

    /// 已提交过 `PlaylistTracks` 请求的歌单(成败都记)。prefetch 据此去重,
    /// 避免**失败**歌单(`tracks` 永远不会被填)被每帧无限重提交而刷屏。
    /// 对齐 cover 的 `covers.pending`。
    pub tracks_requested: FxHashSet<PlaylistId>,

    /// 歌曲 id → 完整结构化歌词(原文 / 逐字 / 翻译 / 罗马音);不在 map 里表示还没拉到 /
    /// 拉失败。channel 层已清洗,client 直接收整份,渲染时按需取各字段。
    pub lyrics: FxHashMap<SongId, Lyrics>,

    /// 各 channel 当前用户喜欢(♥)的歌曲 ID 集合;装饰 `SongView.loved` 用。
    /// 缺 source 时该 source 的歌全部按 `loved=false` 渲染。
    pub liked_ids: FxHashMap<SourceKind, FxHashSet<SongId>>,

    /// 歌曲 id → 远端真实累计播放次数;装饰 `SongView.plays` 用。
    /// 缺 id = 还没查到 / 查失败(渲染成 `None`)。
    pub play_counts: FxHashMap<SongId, u32>,

    /// 已提交过 `RemotePlayCount` 请求的歌曲(成败都记)。停留防抖据此去重,
    /// 避免同一首歌反复打回忆坐标接口。
    pub play_count_requested: FxHashSet<SongId>,
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
            play_counts: FxHashMap::default(),
            play_count_requested: FxHashSet::default(),
        }
    }
}
