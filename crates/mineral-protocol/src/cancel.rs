//! [`CancelFilter`] — IPC 化的批量取消条件。
//!
//! 闭包过不了 wire,但「按种类砍一批」这种谓词足以覆盖现有所有 cancel 场景。
//! 用 enum + tag list 表达。`ChannelFetchKindTag` 已挪到 `mineral-task`(数据
//! owner),本模块从那里 re-export 给 protocol 用户。

use mineral_task::{ChannelFetchKindTag, TaskKind};
use serde::{Deserialize, Serialize};

/// IPC-friendly 的批量取消条件。enum + Vec<tag>,可序列化。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CancelFilter {
    /// 取消所有 [`TaskKind::ChannelFetch`] 任务,且其 `ChannelFetchKind` 命中给定 tag。
    /// 空 vec 等价 no-op。
    ChannelFetchKinds(Vec<ChannelFetchKindTag>),
}

impl CancelFilter {
    /// 给定一个具体 [`TaskKind`],判断本 filter 是否要砍。
    #[must_use]
    pub fn matches(&self, kind: &TaskKind) -> bool {
        match (self, kind) {
            (Self::ChannelFetchKinds(tags), TaskKind::ChannelFetch(k)) => {
                tags.contains(&ChannelFetchKindTag::of(k))
            }
            // 写操作不可批量取消:开跑后远端可能已执行,中途砍只会脱节
            (Self::ChannelFetchKinds(_), TaskKind::PlaylistWrite(_)) => false,
        }
    }
}
