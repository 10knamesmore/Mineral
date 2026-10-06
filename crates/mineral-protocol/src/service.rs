//! Daemon 提供给 client 的操作与统计能力，不包含私有配置。

use serde::{Deserialize, Serialize};

/// 当前 daemon 提供的队列操作与播放统计能力；订阅时重放，能力变更后更新。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// 可执行的队列变换名称，保留 daemon 的声明顺序；名称也是请求中的稳定标识。
    pub queue_transforms: Vec<String>,

    /// 本地播放统计当前的采集可用性
    pub play_counts: PlayCountAvailability,
}

/// 播放统计的当前采集策略；client 据此决定是否请求和展示本地统计。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayCountAvailability {
    /// 数据库可用且 daemon 当前启用播放采集时为 true。
    pub enabled: bool,

    /// 不采集播放统计的来源名称。
    pub excluded_sources: Vec<String>,
}
