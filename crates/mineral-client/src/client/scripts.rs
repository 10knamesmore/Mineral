//! Daemon 逐曲持久值读写。

use mineral_model::SongId;

use super::Client;
use crate::operation::{Outcome, decode_applied, decode_query};

impl Client {
    /// 读 per-song 持久值。
    ///
    /// # Params:
    ///   - `song`: 目标歌
    ///   - `key`: 开放键
    pub async fn store_get(
        &self,
        song: SongId,
        key: &str,
    ) -> Outcome<mineral_protocol::StoreValue> {
        self.request(
            mineral_protocol::Request::StoreGet {
                song,
                key: key.to_owned(),
            },
            |result, request_name| {
                decode_query(result, request_name, |response| match response {
                    mineral_protocol::Response::StoreValue(value) => Some(value),
                    _ => None,
                })
            },
        )
        .await
    }

    /// 写 per-song 持久值。
    ///
    /// # Params:
    ///   - `song`: 目标歌
    ///   - `key`: 开放键
    ///   - `value`: 标量值
    pub async fn store_set(
        &self,
        song: SongId,
        key: &str,
        value: mineral_protocol::StoreValue,
    ) -> Outcome<()> {
        self.request(
            mineral_protocol::Request::StoreSet {
                song,
                key: key.to_owned(),
                value,
            },
            decode_applied,
        )
        .await
    }
}
