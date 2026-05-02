//! 当前登录用户作用域下的协议结构。

use serde::Deserialize;

/// `/weapi/song/like/get`(likelist 别名)响应:当前用户喜欢的歌曲 ID 列表。
#[derive(Debug, Clone, Deserialize)]
pub struct LikeListResp {
    /// 喜欢的歌曲 ID 数组。网易返回数字。
    #[serde(default)]
    pub ids: Vec<i64>,
}
