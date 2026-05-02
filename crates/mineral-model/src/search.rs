use serde::{Deserialize, Serialize};

/// 搜索接口要返回的目标类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    /// 单曲。
    Song,
    /// 专辑。
    Album,
    /// 艺人。
    Artist,
    /// 歌单。
    Playlist,
    /// 用户。
    User,
}
