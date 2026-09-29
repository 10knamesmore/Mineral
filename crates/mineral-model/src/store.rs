//! Scalar song-storage values shared with the IPC protocol.

use serde::{Deserialize, Serialize};

/// per-song 持久 KV 的标量值。
///
/// `Nil` 表示「未设置过该 key」(读未命中)或「删除该 key」(写入时)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StoreValue {
    /// 整数(`local_play_count` / `rating` 等一等字段亦走它)。
    Int(i64),

    /// 浮点。
    Real(f64),

    /// 文本。
    Text(String),

    /// 布尔。
    Bool(bool),

    /// 缺失(读:未命中;写:删除该 key)。
    Nil,
}
