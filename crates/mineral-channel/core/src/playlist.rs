//! 歌单加载意图及其结果；批次大小和远端请求方式由 channel 决定。

use mineral_model::Playlist;
use serde::{Deserialize, Serialize};

/// 调用方需要首批、下一批或整个歌单的曲目。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlaylistLoad {
    /// 只准备首批曲目。
    Preview,

    /// 在上次检查到的位置之后再取一批；结果返回到这一批结束的累计曲目快照。
    More {
        /// 来源返回的下一批起点，不能用实际返回曲目数推算。
        offset: u64,
    },

    /// 遍历整个歌单的歌曲身份，补齐可获取的详情；请求失败须返回错误。
    Complete,
}

/// 一次歌单加载的结果。完整性由 channel 的请求覆盖范围判定，不能用曲目数推测。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaylistDetail {
    /// 元信息及已经获取的曲目，保留来源提供的顺序和重复项。
    pub playlist: Playlist,

    /// 是否已检查整个歌单；远端明确不返回详情的条目可以缺席。
    pub complete: bool,

    /// 下一批的起点；完整结果或无法确认远端顺序的旧缓存没有续页位置。
    pub next_offset: Option<u64>,
}

impl PlaylistDetail {
    /// 所有曲目均已检查的结果，也适用于空歌单。
    pub fn complete(playlist: Playlist) -> Self {
        Self {
            playlist,
            complete: true,
            next_offset: None,
        }
    }
}
