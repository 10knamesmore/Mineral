//! daemon 进程信息、来源能力查询与关闭请求。

use mineral_channel_core::ChannelCaps;
use mineral_model::SourceKind;

use super::Client;
use crate::operation::{Outcome, decode_applied, decode_query};

impl Client {
    /// 拉取各源能力表。
    pub async fn channel_caps(&self) -> Outcome<Vec<(SourceKind, ChannelCaps)>> {
        self.request(
            mineral_protocol::Request::ChannelCaps,
            |result, request_name| {
                decode_query(result, request_name, |response| match response {
                    mineral_protocol::Response::ChannelCaps(caps) => Some(caps),
                    _ => None,
                })
            },
        )
        .await
    }

    /// 请求 daemon 优雅退出并等待确认(CLI `mineral stop` 用)。
    pub async fn shutdown(&self) -> Outcome<()> {
        self.request(mineral_protocol::Request::Shutdown, decode_applied)
            .await
    }

    /// 查询 daemon 进程信息。
    pub async fn daemon_info(&self) -> Outcome<u32> {
        self.request(
            mineral_protocol::Request::DaemonInfo,
            |result, request_name| {
                decode_query(result, request_name, |response| match response {
                    mineral_protocol::Response::DaemonInfo { pid } => Some(pid),
                    _ => None,
                })
            },
        )
        .await
    }
}
