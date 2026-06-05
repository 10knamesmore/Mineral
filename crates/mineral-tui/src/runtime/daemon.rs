//! 默认启动模式下的 daemon 拉起 / attach / 退出收尾。
//!
//! `mineral` 直接启动(无 flag)走这里:优先 attach 已有 daemon,没有就 spawn 一个
//! 独立的 `mineral serve` 子进程再 attach。client 退出时是否连带 kill 掉「本次亲手
//! spawn 的」daemon,由配置 `tui.behavior.kill_spawned_daemon_on_exit` 决定。

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use color_eyre::eyre::{WrapErr, bail};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

use crate::runtime::remote::RemoteClient;

/// spawn 后等 daemon bind socket 的轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// spawn 后等 daemon ready 的总超时。
const SPAWN_TIMEOUT: Duration = Duration::from_secs(5);

/// 确保有一个可用 daemon 并连上它。
///
/// 1. 先试连 `socket`:连得上 → attach 已有 daemon,返回 `(client, None)`。
/// 2. 连不上 → spawn 独立 `mineral serve` 子进程,轮询重连直到 ready,返回
///    `(client, Some(handle))`;超时 [`SPAWN_TIMEOUT`] 仍连不上则 `bail!`。
///
/// # Params:
///   - `socket`: daemon 监听的 unix socket 路径(见 `mineral_paths::socket_path`)。
///   - `kill_on_exit`: client 退出时是否 kill 本次亲手 spawn 的 daemon
///     (配置 `tui.behavior.kill_spawned_daemon_on_exit`;`false` = 续命后台播放)。
///
/// # Return:
///   连好的 [`RemoteClient`] + 可选的 [`DaemonHandle`](仅自己 spawn 时 `Some`)。
pub(crate) async fn ensure(
    socket: &Path,
    kill_on_exit: bool,
) -> color_eyre::Result<(RemoteClient, Option<DaemonHandle>)> {
    if let Ok(client) = RemoteClient::connect(socket).await {
        mineral_log::info!(target: "daemon", "attached to existing daemon");
        return Ok((client, None));
    }
    mineral_log::info!(target: "daemon", "no daemon running, spawning one");
    let mut child = spawn_daemon()?;
    let pid = child.id();
    let client = connect_with_retry(socket, &mut child).await?;
    mineral_log::info!(target: "daemon", pid, "spawned daemon ready");
    Ok((
        client,
        Some(DaemonHandle {
            child,
            kill_on_exit,
        }),
    ))
}

/// 退出时是否应当结束 daemon:仅「本次亲手 spawn」且配置开关为 true。
///
/// # Params:
///   - `kill_on_exit`: 配置开关(`tui.behavior.kill_spawned_daemon_on_exit`)
///   - `spawned`: 本次是否亲手 spawn(attach 已有 daemon 为 false)
///
/// # Return:
///   true = SIGTERM 结束 daemon;false = detach 续命 / 不归我们管。
pub(crate) fn should_kill_spawned(kill_on_exit: bool, spawned: bool) -> bool {
    kill_on_exit && spawned
}

/// spawn 后轮询重连,直到 daemon bind 好 socket。
///
/// 每轮额外 `try_wait` 看 daemon 子进程是否已经退出——退了就立刻捞它的 stderr 把**真因**
/// 内联进报错(不再干等到超时),避免出现「did not become ready」这种无信息量的超时。
async fn connect_with_retry(socket: &Path, child: &mut Child) -> color_eyre::Result<RemoteClient> {
    let deadline = tokio::time::Instant::now() + SPAWN_TIMEOUT;
    loop {
        if let Ok(client) = RemoteClient::connect(socket).await {
            return Ok(client);
        }
        if let Some(status) = child.try_wait().wrap_err("poll spawned daemon")? {
            bail!("{}", daemon_died_report(child, status));
        }
        if tokio::time::Instant::now() >= deadline {
            let hint = mineral_log::log_dir()
                .map_or_else(|_| String::new(), |d| format!(";详见日志 {}", d.display()));
            bail!(
                "spawned daemon did not become ready within {}s (socket {}){hint}",
                SPAWN_TIMEOUT.as_secs(),
                socket.display()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// daemon 子进程启动即退出时,组装一条带它 stderr 真因的报错。
///
/// daemon 以 `stderr(piped)` spawn,退出后管道已关,读取不会阻塞;color-eyre 在非 tty
/// 下输出无 ANSI,正好原样转述给用户。stderr 为空时退回「详见日志」提示。
fn daemon_died_report(child: &mut Child, status: ExitStatus) -> String {
    let mut captured = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut captured);
    }
    let trimmed = captured.trim();
    if trimmed.is_empty() {
        let hint = mineral_log::log_dir()
            .map_or_else(|_| String::new(), |d| format!(",详见日志 {}", d.display()));
        format!("daemon 启动即退出({status}),无 stderr 输出{hint}")
    } else {
        format!("daemon 启动即退出({status}):\n{trimmed}")
    }
}

/// spawn 一个独立的 `mineral serve` 子进程(同一个二进制,自带相同 feature)。
///
/// stdin/stdout null 掉:daemon 的 `println!("listening...")` 不能漏进 TUI 的 alternate
/// screen。**stderr 改 `piped`**:daemon 正常运行不写 stderr(日志走滚动文件),只有启动
/// 失败时 color-eyre 报告会落到这里,被 [`daemon_died_report`] 捞出来内联进错误。
fn spawn_daemon() -> color_eyre::Result<Child> {
    let exe = std::env::current_exe().wrap_err("locate current executable for daemon spawn")?;
    Command::new(&exe)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .wrap_err_with(|| format!("spawn daemon: {} serve", exe.display()))
}

/// 本次启动亲手 spawn 的 daemon 子进程句柄。持有它 = 退出时按配置开关决定 daemon 去留。
pub(crate) struct DaemonHandle {
    /// spawn 出来的 `mineral serve` 子进程。
    child: Child,

    /// client 退出时是否 kill(配置 `tui.behavior.kill_spawned_daemon_on_exit`)。
    kill_on_exit: bool,
}

impl DaemonHandle {
    /// client 退出后调用:按配置开关决定是否结束 daemon。
    ///
    /// kill 走 SIGTERM(而非 SIGKILL)以触发 daemon 的 graceful 收尾(停 audio
    /// engine、清 socket 文件),再 `wait` 回收僵尸进程。`false` 时直接 detach:
    /// 父进程退出后子进程被 init 收养,继续后台播放。
    pub(crate) fn shutdown_if_owned(mut self) {
        if !should_kill_spawned(self.kill_on_exit, /*spawned*/ true) {
            mineral_log::info!(target: "daemon", pid = self.child.id(), "detaching spawned daemon, keeps playing");
            return;
        }
        let pid = match i32::try_from(self.child.id()) {
            Ok(raw) => Pid::from_raw(raw),
            Err(e) => {
                mineral_log::warn!(target: "daemon", error = mineral_log::chain(e), "daemon pid out of range, cannot signal");
                return;
            }
        };
        mineral_log::info!(target: "daemon", pid = pid.as_raw(), "killing spawned daemon on exit");
        if let Err(e) = signal::kill(pid, Signal::SIGTERM) {
            mineral_log::warn!(target: "daemon", error = mineral_log::chain(e), "send SIGTERM to daemon failed");
        }
        if let Err(e) = self.child.wait() {
            mineral_log::warn!(target: "daemon", error = mineral_log::chain(&e), "wait for daemon exit failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::should_kill_spawned;

    /// 开关 × 是否自拉起的判定矩阵:仅「开关开 + 亲手 spawn」才杀。
    #[test]
    fn kill_matrix() {
        assert!(
            !should_kill_spawned(/*kill_on_exit*/ false, /*spawned*/ true),
            "续命开关关 → 不杀"
        );
        assert!(
            should_kill_spawned(/*kill_on_exit*/ true, /*spawned*/ true),
            "默认:自拉起 + 开关开 → 杀"
        );
        assert!(
            !should_kill_spawned(/*kill_on_exit*/ true, /*spawned*/ false),
            "attach 别人的 daemon → 永不杀"
        );
        assert!(
            !should_kill_spawned(/*kill_on_exit*/ false, /*spawned*/ false),
            "两否 → 不杀"
        );
    }
}
