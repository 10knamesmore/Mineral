//! 后端事实的本地镜像；不持有组件光标、动画或资源执行器。

use super::{LibraryData, PlayerMirror};
use crate::runtime::playback::Playback;
use mineral_model::SourceKind;
use rustc_hash::FxHashMap;

/// 各界面可共享的领域模型；组件经独立输入借用所需部分。
pub(crate) struct AppModels {
    /// server 数据镜像/拉取缓存(歌单 / 曲目 / 歌词 + ♥/本地完整播放次数装饰)。
    pub(crate) library: LibraryData,

    /// server 权威播放态镜像(在播歌 / 队列 / 洗牌备份 / 同步版本号)。
    pub(crate) player: PlayerMirror,

    /// 播放状态机。
    pub(crate) playback: Playback,

    /// 后台 server scheduler 当前快照(每 tick 由 App 从 `Client::task_snapshot`
    /// 灌入)。**只含**:server 端 ChannelFetch lane(playlists / tracks /
    /// song-url / lyrics / liked)。封面由客户端图片管线管理,
    /// 不在这里。
    /// `by_kind` 给 top_status 显示「pl:N tr:N ...」按 kind 拆分用。
    pub(crate) tasks_snapshot: mineral_task::Snapshot,

    /// Small download summary polled every tick.
    pub(crate) downloads_summary: mineral_protocol::DownloadSummary,

    /// Flat Song download snapshot, refreshed only while Downloads overlay is open.
    pub(crate) downloads: Vec<mineral_protocol::SongDownloadView>,

    /// 各源能力声明镜像(启动时从 server 拉一次)。UI 据此决定渲染哪些入口
    /// (搜索类型 / 歌单写操作键 / 网页链接复制项);缺项 = 该源未注册,入口不画。
    pub(crate) caps: FxHashMap<SourceKind, mineral_channel_core::ChannelCaps>,

    /// Daemon operations and play-count availability; never part of local config.
    pub(crate) service_info: mineral_protocol::ServiceInfo,
}

impl AppModels {
    /// 等待后端握手和事件填充的空镜像。
    pub(crate) fn new() -> Self {
        Self {
            library: LibraryData::new(),
            player: PlayerMirror::new(),
            playback: Playback::new(),
            tasks_snapshot: mineral_task::Snapshot {
                running: 0,
                by_kind: FxHashMap::default(),
            },
            downloads_summary: mineral_protocol::DownloadSummary::default(),
            downloads: Vec::new(),
            caps: FxHashMap::default(),
            service_info: mineral_protocol::ServiceInfo::default(),
        }
    }
}
