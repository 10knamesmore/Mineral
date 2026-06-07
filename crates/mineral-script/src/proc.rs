//! `mineral.spawn` 的子进程执行面:结构化组装 `tokio::process::Command`、
//! 收集输出、支持中途 kill。本模块只管「跑一个子进程」,并发闸与
//! kill 路由在 daemon 泵(server 侧)。

use color_eyre::eyre::WrapErr;

/// 一次 `mineral.spawn` 的结构化参数(Lua table 在 api 层边界解析)。
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    /// 可执行文件。
    pub(crate) program: String,

    /// 参数列表(结构化,不拼 shell 串)。
    pub(crate) args: Vec<String>,

    /// 工作目录;`None` = 继承 daemon cwd。
    pub(crate) cwd: Option<std::path::PathBuf>,

    /// 追加 / 覆盖的环境变量。
    pub(crate) env: Vec<(String, String)>,
}

impl SpawnSpec {
    /// 可执行文件(只读;日志用)。
    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }
}

/// 子进程结束后的结构化结果(回投脚本回调)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnResult {
    /// 退出码;被信号终止(含 kill)时为 `None`。
    pub code: Option<i32>,

    /// 标准输出(UTF-8 lossy)。
    pub stdout: String,

    /// 标准错误(UTF-8 lossy)。
    pub stderr: String,

    /// 是否被脚本 `handle:kill()` 中止。
    pub killed: bool,
}

/// 一次 spawn 在脚本侧的标识(`handle:kill()` 经它路由)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpawnId(pub(crate) u64);

/// 跑一个子进程到结束(或被 kill),收集输出。
///
/// stdout / stderr 由独立任务并发收集(避免管道写满死锁);`kill` 触发时
/// SIGKILL 子进程并等待收尸(不留僵尸),`killed` 置真。
///
/// # Params:
///   - `spec`: 结构化参数
///   - `kill`: 中止信号接收端(发送端 drop 不触发 kill,正常等退出)
///
/// # Return:
///   结构化结果;spawn 本身失败(可执行不存在等)为 `Err`。
pub async fn run_child(
    spec: SpawnSpec,
    kill: tokio::sync::oneshot::Receiver<()>,
) -> color_eyre::Result<SpawnResult> {
    use tokio::io::AsyncReadExt;
    let mut cmd = tokio::process::Command::new(&spec.program);
    cmd.args(&spec.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // 兜底:result 任务被丢(daemon 关停)时随句柄收掉子进程。
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }
    let mut child = cmd
        .spawn()
        .wrap_err_with(|| format!("spawn `{}` 失败", spec.program))?;
    // 先起两条收集任务再等退出:子进程写满管道缓冲会阻塞,必须边跑边读。
    let stdout_task = child.stdout.take().map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf).await;
            buf
        })
    });
    let stderr_task = child.stderr.take().map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf).await;
            buf
        })
    });
    let (status, killed) = tokio::select! {
        status = child.wait() => (status.wrap_err("等待子进程退出失败")?, false),
        _ = kill => {
            child.kill().await.wrap_err("kill 子进程失败")?;
            // kill() 内部已 wait 收尸;再 wait 拿终态(已退出,立即返回)。
            (child.wait().await.wrap_err("kill 后收尸失败")?, true)
        }
    };
    let stdout = collect(stdout_task).await;
    let stderr = collect(stderr_task).await;
    Ok(SpawnResult {
        code: status.code(),
        stdout,
        stderr,
        killed,
    })
}

/// 等一条输出收集任务结束并转成 lossy UTF-8(任务缺席 / 失败给空串)。
async fn collect(task: Option<tokio::task::JoinHandle<Vec<u8>>>) -> String {
    match task {
        Some(handle) => match handle.await {
            Ok(buf) => String::from_utf8_lossy(&buf).into_owned(),
            Err(_join) => String::new(),
        },
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{SpawnResult, SpawnSpec, run_child};

    /// 测试用 spec(无 cwd / env)。
    fn spec(program: &str, args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            program: program.to_owned(),
            args: args.iter().map(|&a| a.to_owned()).collect(),
            cwd: None,
            env: Vec::new(),
        }
    }

    #[tokio::test]
    async fn echo_exits_zero_with_stdout() -> color_eyre::Result<()> {
        let (_kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        let result = run_child(spec("echo", &["hi"]), kill_rx).await?;
        assert_eq!(
            result,
            SpawnResult {
                code: Some(0),
                stdout: "hi\n".to_owned(),
                stderr: String::new(),
                killed: false,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn kill_interrupts_long_running_child() -> color_eyre::Result<()> {
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(run_child(spec("sleep", &["30"]), kill_rx));
        // 给子进程一点起跑时间再 kill。
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        kill_tx
            .send(())
            .map_err(|()| color_eyre::eyre::eyre!("kill 信号发送失败"))?;
        let result = task.await??;
        assert!(result.killed, "kill 后 killed 必须为真");
        assert_eq!(result.code, None, "SIGKILL 终止无退出码");
        Ok(())
    }

    #[tokio::test]
    async fn missing_program_is_spawn_error() {
        let (_kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        let result = run_child(spec("mineral-test-no-such-bin", &[]), kill_rx).await;
        assert!(result.is_err(), "可执行不存在必须报 spawn 错误");
    }

    #[tokio::test]
    async fn env_and_args_reach_child() -> color_eyre::Result<()> {
        let (_kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        let mut s = spec("sh", &["-c", "printf '%s' \"$MINERAL_SPAWN_T\""]);
        s.env.push(("MINERAL_SPAWN_T".to_owned(), "钾".to_owned()));
        let result = run_child(s, kill_rx).await?;
        assert_eq!(result.stdout, "钾");
        Ok(())
    }
}
