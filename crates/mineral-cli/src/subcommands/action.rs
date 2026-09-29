//! `mineral action <name>` — 触发 daemon 内脚本注册的具名动作。
//!
//! 配合系统快捷键工具(sxhkd / Hyprland bind 等)即可把脚本动作绑到全局键;
//! 错误(未注册 / 脚本未启用 / 执行失败)按 daemon 的失败类别报错，内部诊断仅写日志。

use crate::error::{Error, Result};
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
use mineral_protocol::SocketWire;

/// `mineral action` 入口:连 daemon socket(含握手)→ 触发动作 → 按结果退出。
///
/// # Params:
///   - `name`: 动作注册名(config.lua 里 `mineral.action` 的第一个参数)
///   - `args`: 位置实参,原样带给动作回调(Lua 侧 `ctx.args`)
pub async fn run(name: &str, args: &[String]) -> Result<()> {
    let socket_path = mineral_paths::socket_path()?;
    let wire = SocketWire::connect(&socket_path)
        .await
        .map_err(|source| Error::SocketConnect {
            path: socket_path.clone(),
            source,
        })?;
    let client = Client::from_wire(Box::new(wire), "mineral_action", ClientConfig::default())
        .await
        .map_err(|source| Error::Handshake {
            path: socket_path,
            source,
        })?;
    // CLI 无界面,采不到按键上下文。
    match client
        .invoke_action(name, /*ctx*/ None, args.to_vec())
        .await
    {
        Outcome::Applied(()) => {
            println!("action {name:?} done");
            Ok(())
        }
        Outcome::Accepted(()) => {
            println!("action {name:?} accepted");
            Ok(())
        }
        Outcome::Failed { kind, detail } => {
            mineral_log::warn!(target: "cli", action = name, ?kind, detail = %detail, "action rejected by daemon");
            Err(Error::Rejected {
                operation: "invoke action",
                kind,
            })
        }
        Outcome::Unknown { reason } => {
            mineral_log::warn!(target: "cli", action = name, ?reason, "action outcome unknown");
            Err(Error::UnknownOutcome {
                operation: "invoke action",
                reason,
            })
        }
    }
}
