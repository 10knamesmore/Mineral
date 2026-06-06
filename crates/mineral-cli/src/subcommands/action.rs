//! `mineral action <name>` — 触发 daemon 内脚本注册的具名动作。
//!
//! 配合系统快捷键工具(sxhkd / Hyprland bind 等)即可把脚本动作绑到全局键;
//! 错误(未注册 / 脚本未启用 / 执行失败)原样打到 stderr 并非零退出。

use color_eyre::eyre::bail;
use mineral_protocol::{OneshotClient, Request, Response};

/// `mineral action` 入口:连 daemon socket(含握手)→ 触发动作 → 按结果退出。
///
/// # Params:
///   - `name`: 动作注册名(config.lua 里 `mineral.action` 的第一个参数)
pub async fn run(name: &str) -> color_eyre::Result<()> {
    let socket_path = mineral_paths::socket_path()?;
    let mut client = OneshotClient::connect(&socket_path).await?;
    match client
        .request(Request::InvokeAction(name.to_owned()))
        .await?
    {
        Response::Ok => {
            println!("action {name:?} done");
            Ok(())
        }
        Response::Error(msg) => bail!("{msg}"),
        other => bail!("unexpected response: {other:?}"),
    }
}
