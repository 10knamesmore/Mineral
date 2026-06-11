//! 列表浏览态:两个列表各自的光标 + 视口滚动,跨歌单的位置记忆,选中变化时刻。
//!
//! 持有「光标停在哪、视口滚到哪、各歌单上次停在哪」;操作这些字段的导航执行器
//! 因要跨读数据 / 搜索 / 配置域,留在组合根([`AppState`](crate::runtime::state::AppState))上。

use std::time::Instant;

use crate::runtime::scroll::ListScroll;
use crate::runtime::track_pos::{PendingRestore, TrackPosMap};

/// 列表浏览态([`AppState`](crate::runtime::state::AppState) 的导航域)。
pub struct NavState {
    /// Playlists 视图当前选中行。
    pub sel_playlist: usize,

    /// Playlists 列表的视口滚动态(nvim 手感 + 缓动平移)。
    pub scroll_playlist: ListScroll,

    /// Library 视图当前选中行。
    pub sel_track: usize,

    /// Library 列表的视口滚动态。
    pub scroll_track: ListScroll,

    /// 各歌单的光标位置记忆(`behavior.remember_track_pos` 非 off 时退出 Library
    /// 记录、再进恢复;persist 档启动时灌入落盘值)。
    pub track_pos: TrackPosMap,

    /// 进歌单时曲目未就绪而挂起的位置恢复;`PlaylistTracksFetched` 到达时若用户
    /// 还停在该歌单且光标未动过则补落位,否则作废。
    pub pending_track_restore: Option<PendingRestore>,

    /// 上一次选中行变化的时间(navigation key 命中时刷新)。cover_image 用它做
    /// 防抖:连续滚动时跳过昂贵的 protocol 构建,稳态后再上图。
    pub last_sel_change: Instant,
}

impl NavState {
    /// 构造初始浏览态(光标在首行、视口未滚、无位置记忆)。
    pub(crate) fn new() -> Self {
        Self {
            sel_playlist: 0,
            scroll_playlist: ListScroll::new(),
            sel_track: 0,
            scroll_track: ListScroll::new(),
            track_pos: TrackPosMap::default(),
            pending_track_restore: None,
            last_sel_change: Instant::now(),
        }
    }
}
