//! 用户保存的歌单操作。

use mineral_model::PlaylistId;
use serde::{Deserialize, Serialize};

/// 保存时由 daemon 读取完整队列；管理操作以身份定位。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaylistOp {
    /// 新建独立歌单，允许同名。
    SaveQueue {
        /// 新歌单名称。
        name: String,
    },

    /// 修改自建歌单名。
    Rename {
        /// 目标歌单身份。
        id: PlaylistId,

        /// 新名称。
        name: String,
    },

    /// 删除自建歌单。
    Delete {
        /// 目标歌单身份。
        id: PlaylistId,
    },
}
