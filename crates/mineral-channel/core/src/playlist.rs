//! 歌单流产出的累计快照；新鲜度与完整性分别表达。

use futures_util::StreamExt;
use mineral_model::Playlist;
use serde::{Deserialize, Serialize};

use crate::{Error, FetchSnapshot, FetchStream, Result};

/// 一次歌单加载的结果。完整性由 channel 的请求覆盖范围判定，不能用曲目数推测。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaylistDetail {
    /// 元信息及已经获取的曲目，保留来源提供的顺序和重复项。
    pub playlist: Playlist,

    /// 是否已检查整个歌单；远端明确不返回详情的条目可以缺席。
    pub complete: bool,
}

impl PlaylistDetail {
    /// 所有曲目均已检查的结果，也适用于空歌单。
    pub fn complete(playlist: Playlist) -> Self {
        Self {
            playlist,
            complete: true,
        }
    }
}

/// 等待完整的新鲜歌单；中途错误或缺少完整结果的 EOF 都返回失败。
pub async fn finish_playlist(
    mut snapshots: FetchStream<'_, PlaylistDetail>,
) -> Result<PlaylistDetail> {
    while let Some(snapshot) = snapshots.next().await {
        if let FetchSnapshot::Fresh(detail) = snapshot?
            && detail.complete
        {
            return Ok(detail);
        }
    }
    Err(Error::InvalidData {
        field: "playlist stream ended before complete",
    })
}
