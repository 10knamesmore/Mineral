//! daemon 进程级 e2e:用真 `mineral` 二进制(`CARGO_BIN_EXE_mineral`)起 daemon,
//! 验证 bind / stale 检测 / SIGTERM graceful 清 socket。
//!
//! 不需要 TUI / pty —— 只起 `mineral serve` 子进程,用 unix socket 探测;每个测试
//! 隔离一套临时 XDG 目录,互不干扰、可并行。
//!
//! 注:audio engine 拿不到设备时**降级到 null 模式**(引擎空跑、daemon 照常 bind /
//! serve / graceful shutdown),所以这些用例在 headless CI 上也稳跑,不依赖真音频栈。
//! `daemon_status_reports_null_backend` 进一步用 `MINERAL_AUDIO_NULL` 强制降级,
//! 在有声卡的开发机上也能确定性验证「engine null → IPC → CLI status 感知」整条链。

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, bail};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// 隔离环境里的一个 daemon 子进程;Drop 时 kill 子进程并清临时目录。
struct Daemon {
    /// `mineral serve` 子进程。
    child: Child,

    /// 隔离用的临时根目录(XDG_* 全指到这下面)。
    root: PathBuf,

    /// daemon 监听的 socket 路径(`<runtime>/mineral/mineral.sock`)。
    socket: PathBuf,
}

impl Daemon {
    /// 起一个隔离环境的 daemon 子进程(不等它 ready)。
    fn spawn(tag: &str) -> color_eyre::Result<Self> {
        Self::spawn_inner(tag, /*force_null*/ false)
    }

    /// 起一个**强制 null 音频后端**的 daemon(无视本机有无声卡),用于确定性验证降级。
    fn spawn_null(tag: &str) -> color_eyre::Result<Self> {
        Self::spawn_inner(tag, /*force_null*/ true)
    }

    /// `spawn` / `spawn_null` 的共同实现。
    fn spawn_inner(tag: &str, force_null: bool) -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-e2e-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        let runtime = root.join("runtime");
        std::fs::create_dir_all(&runtime).wrap_err("create isolated runtime dir")?;
        let mut cmd = serve_command(&root);
        if force_null {
            cmd.env("MINERAL_AUDIO_NULL", "1");
        }
        let child = cmd.spawn().wrap_err("spawn `mineral serve`")?;
        let socket = runtime.join("mineral/mineral.sock");
        Ok(Self {
            child,
            root,
            socket,
        })
    }

    /// 在同一隔离 XDG 环境下跑 `mineral status`,捕获其输出。
    fn status_output(&self) -> color_eyre::Result<std::process::Output> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mineral"));
        cmd.arg("status")
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .stdin(Stdio::null());
        cmd.output().wrap_err("run `mineral status`")
    }

    /// 轮询直到 socket 可连(daemon ready),超时则报错。
    fn wait_ready(&self) -> color_eyre::Result<()> {
        wait_until(Duration::from_secs(10), || {
            UnixStream::connect(&self.socket).is_ok()
        })
        .wrap_err("daemon did not become ready in time")
    }

    /// 给 daemon 发 SIGTERM(走它的 graceful shutdown 路径)。
    fn sigterm(&self) -> color_eyre::Result<()> {
        let pid = i32::try_from(self.child.id()).wrap_err("daemon pid out of range")?;
        kill(Pid::from_raw(pid), Signal::SIGTERM).wrap_err("send SIGTERM to daemon")
    }

    /// 阻塞等 daemon 进程退出。退出意味着 log guard 已 drop → 滚动日志 flush 完毕,
    /// 之后读日志文件才拿得到关停那几行。
    fn wait_for_exit(&mut self) -> color_eyre::Result<()> {
        self.child.wait().wrap_err("wait daemon exit")?;
        Ok(())
    }

    /// 读这个 daemon 隔离 cache 目录下的全部滚动日志内容拼成一个 String。
    fn read_logs(&self) -> color_eyre::Result<String> {
        let dir = self.root.join("cache/mineral");
        let mut out = String::new();
        for entry in std::fs::read_dir(&dir).wrap_err("read daemon log dir")? {
            let path = entry.wrap_err("log dir entry")?.path();
            let is_log = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("mineral.log"));
            if is_log {
                out.push_str(&std::fs::read_to_string(&path).wrap_err("read log file")?);
            }
        }
        Ok(out)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // 兜底清理:SIGKILL(测试若已 graceful 退,kill 命中 zombie 也无妨)+ 收尸 + 删目录。
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 构造一条隔离 XDG 环境的 `mineral serve` 命令(stdio 全 null)。
fn serve_command(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mineral"));
    cmd.arg("serve")
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// 纳秒时间戳,给临时目录名做唯一后缀。
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 轮询 `cond` 直到为真或超时。
fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> color_eyre::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("condition not met within {timeout:?}")
}

/// daemon 起来后 socket 可连;收到 SIGTERM 后 graceful 退出并 unlink socket 文件。
#[test]
fn daemon_binds_then_cleans_socket_on_sigterm() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("graceful")?;
    daemon.wait_ready()?;
    assert!(
        UnixStream::connect(&daemon.socket).is_ok(),
        "ready 之后 socket 应可连"
    );

    daemon.sigterm()?;
    wait_until(Duration::from_secs(10), || !daemon.socket.exists())
        .wrap_err("socket 未在 SIGTERM 后被清理")?;
    assert!(
        !daemon.socket.exists(),
        "graceful shutdown 应 unlink socket 文件"
    );
    Ok(())
}

/// 已有 daemon 在跑时,第二个 `mineral serve`(撞同一 socket)应被拒绝、非零退出,
/// 且不影响第一个 daemon。
#[test]
fn second_daemon_is_refused() -> color_eyre::Result<()> {
    let first = Daemon::spawn("refuse")?;
    first.wait_ready()?;

    // 复用 first 的隔离环境起第二个 → 撞同一个 socket。
    let status = serve_command(&first.root)
        .status()
        .wrap_err("run second `mineral serve`")?;
    assert!(
        !status.success(),
        "已有 daemon 时第二个应被拒绝(非零退出),实际 {status:?}"
    );
    assert!(
        UnixStream::connect(&first.socket).is_ok(),
        "第一个 daemon 应仍在跑"
    );
    Ok(())
}

/// server 被 `kill`(SIGTERM)时应记关停日志,不是 silent dead。
/// (SIGKILL 物理上无法打日志,不在此覆盖。)
#[test]
fn daemon_logs_shutdown_on_sigterm() -> color_eyre::Result<()> {
    let mut daemon = Daemon::spawn("logterm")?;
    daemon.wait_ready()?;

    daemon.sigterm()?;
    daemon.wait_for_exit()?;

    let logs = daemon.read_logs()?;
    assert!(
        logs.contains("shutdown signal received") && logs.contains("shutting down"),
        "daemon 应记录收到信号 + 关停日志,实际:\n{logs}"
    );
    Ok(())
}

/// 强制 null 后端的 daemon:`mineral status` 应连上、退出码 0、且报告 backend 降级。
/// 覆盖「engine null → AudioSnapshot.backend → IPC → CLI status 感知」整条链。
#[test]
fn daemon_status_reports_null_backend() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn_null("nullstatus")?;

    // 不做独立的 wait_ready 探测连接 —— daemon 是单 client 设计(serve.rs),探测连接
    // 释放 busy 标志的短窗口会跟紧接的 `mineral status` 撞成「daemon busy」。改成直接
    // 重试 status:它本身就是就绪探测,连不上(daemon 未起)/ 瞬时 busy 都重试到成功,
    // 全程只有这一个 client,无竞态。
    let deadline = Instant::now() + Duration::from_secs(10);
    let stdout = loop {
        let out = daemon.status_output()?;
        if out.status.success() {
            break String::from_utf8_lossy(&out.stdout).into_owned();
        }
        if Instant::now() >= deadline {
            bail!(
                "status 始终未成功退出,最后 stderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        stdout.contains("backend:    null (no audio device)"),
        "status 应报告 null 后端,实际 stdout:\n{stdout}"
    );
    Ok(())
}
