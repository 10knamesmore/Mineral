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
///   - `args`: 位置实参,原样带给动作回调(Lua 侧 `ctx.args`)
pub async fn run(name: &str, args: &[String]) -> color_eyre::Result<()> {
    let socket_path = mineral_paths::socket_path()?;
    let mut client = OneshotClient::connect(&socket_path).await?;
    match client
        .request(Request::InvokeAction {
            name: name.to_owned(),
            ctx: None, // CLI 无界面,采不到按键上下文
            args: args.to_vec(),
        })
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
