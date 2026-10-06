//! 查询启动所需的来源能力与 daemon 服务能力。

use super::Client;
use crate::connection::Bootstrap;
use crate::operation::{Outcome, decode_query};

impl Client {
    /// 拉一次启动自举能力表,CLI / TUI 连接后调用。
    pub async fn bootstrap(&self) -> Bootstrap {
        let caps = bootstrap_value(self.channel_caps().await, "channel_caps");
        let service_info = bootstrap_value(self.service_info().await, "service_info");
        Bootstrap { caps, service_info }
    }

    /// 查询当前 daemon 的队列变换与播放统计能力，不读取其配置。
    pub async fn service_info(&self) -> Outcome<mineral_protocol::ServiceInfo> {
        self.request(mineral_protocol::Request::ServiceInfo, |result, name| {
            decode_query(result, name, |response| match response {
                mineral_protocol::Response::ServiceInfo(info) => Some(info),
                _ => None,
            })
        })
        .await
    }
}

/// 自举查询失败时记录类别与操作，然后维持空结果降级语义。
fn bootstrap_value<T: Default>(outcome: Outcome<T>, operation: &'static str) -> T {
    match outcome {
        Outcome::Applied(value) | Outcome::Accepted(value) => value,
        Outcome::Failed { kind, detail } => {
            mineral_log::warn!(
                target: "ipc",
                operation,
                kind = ?kind,
                detail,
                "自举查询被 daemon 拒绝,使用空结果"
            );
            T::default()
        }
        Outcome::Unknown { reason } => {
            mineral_log::warn!(
                target: "ipc",
                operation,
                reason = ?reason,
                error = mineral_log::chain(&reason),
                "自举查询结果未知,使用空结果"
            );
            T::default()
        }
    }
}
