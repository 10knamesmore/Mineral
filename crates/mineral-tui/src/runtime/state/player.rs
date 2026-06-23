//! server 权威播放态的 client 端镜像:在播歌、播放队列、洗牌备份、同步版本号。
//!
//! 每 tick 由 `apply_player_sync` 按版本门控灌入(重段缺席 = 与已有一致、原地保持)。
//! 区别于 [`Playback`](crate::runtime::playback::Playback):后者是本地播放状态机
//! (playing/paused/position/volume,由 audio snapshot 驱动);本域是「队列 + 在播 +
//! 同步簿记」,由 server 的 `PlayerSync` 驱动。

use mineral_model::Song;

/// server 播放态镜像([`AppState`](crate::runtime::state::AppState) 的同步域)。
pub struct PlayerMirror {
    /// 当前正在播放(用于 Library 视图行首 ♫ 标记)。
    pub current: Option<Song>,

    /// 浮动 queue 当前曲目列表(后端权威态)。
    pub queue: Vec<Song>,

    /// server 的「在播位置锚点」:`queue` 中当前在播歌所在的下标(prev/next 由它推进)。
    /// 渲染 queue 浮层时按此下标标 `▶`,而非按歌曲身份——队列含重复曲时身份匹配会
    /// 把全部副本一起点亮,只有下标能精确指出真正在播的那一行。
    pub queue_sel: usize,

    /// Shuffle 状态下保存的原始 queue 顺序。退 Shuffle 时还原。
    /// 非 Shuffle 状态恒为 `None`。
    pub original_queue: Option<Vec<Song>>,

    /// 上次已应用的 server 状态版本号(每 tick 随 PlayerSync 回报;0 = 还没同步过,
    /// 首次同步必然全量)。
    pub versions: mineral_protocol::PlayerVersions,
}

impl PlayerMirror {
    /// 构造空镜像(无在播、空队列、版本归零)。
    pub(crate) fn new() -> Self {
        Self {
            current: None,
            queue: Vec::new(),
            queue_sel: 0,
            original_queue: None,
            versions: mineral_protocol::PlayerVersions::default(),
        }
    }
}
