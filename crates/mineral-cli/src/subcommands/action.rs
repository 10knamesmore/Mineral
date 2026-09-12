//! `mineral action <name>` — 触发 daemon 内脚本注册的具名动作。
//!
//! 配合系统快捷键工具(sxhkd / Hyprland bind 等)即可把脚本动作绑到全局键;
//! 错误(未注册 / 脚本未启用 / 执行失败)原样打到 stderr 并非零退出。

use color_eyre::eyre::bail;
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
use mineral_protocol::SocketWire;

/// `mineral action` 入口:连 daemon socket(含握手)→ 触发动作 → 按结果退出。
///
/// # Params:
///   - `name`: 动作注册名(config.lua 里 `mineral.action` 的第一个参数)
///   - `args`: 位置实参,原样带给动作回调(Lua 侧 `ctx.args`)
pub async fn run(name: &str, args: &[String]) -> color_eyre::Result<()> {
    let socket_path = mineral_paths::socket_path()?;
    let wire = SocketWire::connect(&socket_path).await?;
    let client =
        Client::from_wire(Box::new(wire), "mineral_action", ClientConfig::default()).await?;
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
        Outcome::Failed { detail, .. } | Outcome::Unknown { detail } => bail!("{detail}"),
    }
}
