//! 查询启动所需的来源能力与脚本绑定。

use super::Client;
use crate::connection::Bootstrap;
use crate::operation::Outcome;

impl Client {
    /// 拉一次启动自举数据(能力表 + 脚本绑定),CLI / TUI 连接后调用。
    pub async fn bootstrap(&self) -> Bootstrap {
        let channel_caps = bootstrap_value(self.channel_caps().await, "channel_caps");
        let script_binds = bootstrap_value(self.script_binds().await, "script_binds");
        Bootstrap {
            channel_caps,
            script_binds,
        }
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
