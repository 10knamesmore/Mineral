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

    /// 隔离用的临时根目录(XDG_CONFIG/DATA/CACHE 全指到这下面)。
    root: PathBuf,

    /// socket 目录(经 `MINERAL_SOCKET_DIR` 注入)。与 `root` 分开且**短**:macOS 的
    /// `$TMPDIR` 本就很长,socket 全路径要压在 `sun_path`(104)以内,不能嵌进长名的 `root`。
    sock_dir: PathBuf,

    /// daemon 监听的 socket 路径(`<sock_dir>/mineral.sock`)。
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

    /// 起一个**注定启动失败**的 daemon:预埋一份旧透明格式的网易云凭证
    /// (`user_id` 是裸字符串),结构化后的 `UserId` 解不出来 → daemon 在 build channels
    /// 阶段就 `Err` 退出。用于验证「启动失败被写进日志 / 被 client 看见」。
    fn spawn_failing_credential(tag: &str) -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-e2e-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        let sock_dir = short_sock_dir();
        let cred_dir = root.join("data/mineral");
        std::fs::create_dir_all(&cred_dir).wrap_err("create credential dir")?;
        // 旧格式:user_id 是裸字符串(结构化前的 `#[serde(transparent)]` 形态)。
        std::fs::write(
            cred_dir.join("netease.json"),
            r#"{"music_u":"opaque-token","user_id":"349758847"}"#,
        )
        .wrap_err("seed legacy credential")?;
        let child = serve_command(&root, &sock_dir)
            .spawn()
            .wrap_err("spawn failing `mineral serve`")?;
        let socket = sock_dir.join("mineral.sock");
        Ok(Self {
            child,
            root,
            sock_dir,
            socket,
        })
    }

    /// `spawn` / `spawn_null` 的共同实现。
    fn spawn_inner(tag: &str, force_null: bool) -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-e2e-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        let sock_dir = short_sock_dir();
        std::fs::create_dir_all(&root).wrap_err("create isolated root dir")?;
        let mut cmd = serve_command(&root, &sock_dir);
        if force_null {
            cmd.env("MINERAL_AUDIO_NULL", "1");
        }
        let child = cmd.spawn().wrap_err("spawn `mineral serve`")?;
        let socket = sock_dir.join("mineral.sock");
        Ok(Self {
            child,
            root,
            sock_dir,
            socket,
        })
    }

    /// 在同一隔离环境下跑 `mineral status`,捕获其输出(与 daemon 共享 `MINERAL_SOCKET_DIR`
    /// 才能连上同一 socket)。
    fn status_output(&self) -> color_eyre::Result<std::process::Output> {
        self.cli_output("status")
    }

    /// 在同一隔离环境下跑 `mineral stop`,捕获其输出。
    fn stop_output(&self) -> color_eyre::Result<std::process::Output> {
        self.cli_output("stop")
    }

    /// 在同一隔离环境下跑任意单参数 CLI 子命令(与 daemon 共享 `MINERAL_SOCKET_DIR`
    /// 才能连上同一 socket)。
    fn cli_output(&self, subcommand: &str) -> color_eyre::Result<std::process::Output> {
        self.cli_args_output(&[subcommand])
    }

    /// 在同一隔离环境下跑多参数 CLI(如 `stats status`);共享 `XDG_DATA_HOME` 故读到
    /// daemon 写的同一 stats.db。`stats` 走离线直读、不连 socket,与 daemon 无 busy 竞争。
    fn cli_args_output(&self, args: &[&str]) -> color_eyre::Result<std::process::Output> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mineral"));
        cmd.args(args)
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("MINERAL_SOCKET_DIR", &self.sock_dir)
            .stdin(Stdio::null());
        cmd.output()
            .wrap_err_with(|| format!("run `mineral {}`", args.join(" ")))
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
        // 兜底清理:SIGKILL(测试若已 graceful 退,kill 命中 zombie 也无妨)+ 收尸 + 删两处临时目录。
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.sock_dir);
    }
}

/// 短 socket 目录 `$TMPDIR/mnl-<pid>-<suffix>`:刻意**短**,跟长名 `root` 分开,
/// 让 socket 全路径压在 `sun_path`(macOS 104)以内。
fn short_sock_dir() -> PathBuf {
    std::env::temp_dir().join(format!("mnl-{}-{}", std::process::id(), unique_suffix()))
}

/// 构造一条隔离环境的 `mineral serve` 命令:XDG 配置/数据/缓存指向 `root`,socket 经
/// `MINERAL_SOCKET_DIR` 指向 `sock_dir`(两端共用即连同一 socket)。stdio 全 null。
fn serve_command(root: &std::path::Path, sock_dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mineral"));
    cmd.arg("serve")
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("MINERAL_SOCKET_DIR", sock_dir)
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

    // 复用 first 的隔离环境(含同一 sock_dir)起第二个 → 撞同一个 socket。
    let status = serve_command(&first.root, &first.sock_dir)
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

/// 单个 channel 凭证损坏时,daemon **不该整体退出**——只跳过该源 + 记 warn,照常 bind /
/// serve。验证「单 channel 失败不阻塞 daemon」。
#[test]
fn daemon_survives_corrupt_netease_credential() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn_failing_credential("badcred")?;

    // 单 client 设计:不做独立 wait_ready 探测(探测连接释放 busy 的窗口会跟紧接的
    // status 撞成 busy)。直接重试 status —— 它本身就是就绪探测,连不上 / 瞬时 busy
    // 都重试到成功。能成功即证明坏凭证下 daemon 仍 bind + serve。
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if daemon.status_output()?.status.success() {
            break;
        }
        if Instant::now() >= deadline {
            bail!("坏 netease 凭证下 daemon 始终未能正常 serve(应跳过该源照常起)");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let logs = daemon.read_logs()?;
    assert!(
        logs.contains("netease channel 构建失败,跳过"),
        "应记 warn 跳过该源(而非整体崩),实际:\n{logs}"
    );
    Ok(())
}

/// `mineral stop`:经 IPC 请求 daemon 优雅退出。覆盖「CLI → Request::Shutdown →
/// dispatch → serve select → graceful 收尾」整条链:进程退出、socket 被清理、
/// 日志可见关停原因。
#[test]
fn daemon_stops_on_ipc_shutdown() -> color_eyre::Result<()> {
    let mut daemon = Daemon::spawn_null("ipcstop")?;

    // 与 status 测试相反,stop **必须**先 wait_ready:它对「连不上」是幂等成功
    // (确保不在跑的语义),daemon 未 bind 时拿它当就绪探测会假阳性通过。
    // ready 之后再重试 —— 探测连接释放 busy 的短窗里 stop 可能被拒,重试兜住。
    daemon.wait_ready()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = daemon.stop_output()?;
        if out.status.success() {
            break;
        }
        if Instant::now() >= deadline {
            bail!(
                "stop 始终未成功,最后 stderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    daemon.wait_for_exit()?;
    assert!(
        !daemon.socket.exists(),
        "IPC shutdown 应与 SIGTERM 同样 unlink socket 文件"
    );
    let logs = daemon.read_logs()?;
    assert!(
        logs.contains("shutdown requested via IPC") && logs.contains("shutting down"),
        "daemon 应记录 IPC 关停请求 + 关停日志,实际:\n{logs}"
    );
    Ok(())
}

/// daemon 没在跑时 `mineral stop` 是幂等成功(exit 0 + 人话提示):语义是
/// 「确保 daemon 不在跑」,脚本里可无脑调用。
#[test]
fn stop_without_daemon_succeeds_idempotently() -> color_eyre::Result<()> {
    // 隔离环境但**不**起 daemon:socket 目录存在而 socket 文件不存在。
    let root = std::env::temp_dir().join(format!(
        "mineral-e2e-stopidem-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let sock_dir = short_sock_dir();
    std::fs::create_dir_all(&root).wrap_err("create isolated root dir")?;
    let out = Command::new(env!("CARGO_BIN_EXE_mineral"))
        .arg("stop")
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("MINERAL_SOCKET_DIR", &sock_dir)
        .stdin(Stdio::null())
        .output()
        .wrap_err("run `mineral stop`")?;
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&sock_dir);

    assert!(
        out.status.success(),
        "无 daemon 时 stop 应幂等成功,实际 {:?},stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("没有在跑的 daemon"),
        "应人话提示无 daemon,实际 stdout:\n{stdout}"
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

/// 从 `stats status` 输出末行解析 `events: N`(格式 `plays: A   sessions: B   events: C`)。
fn parse_events(stdout: &str) -> Option<i64> {
    stdout
        .rsplit("events:")
        .next()
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|n| n.parse::<i64>().ok())
}

/// 端到端:真 daemon 起来写 app_lifecycle start(Server::spawn),SIGTERM graceful
/// 关停 flush 写 stop —— `stats status` 读同一 stats.db,start 后 events≥1、stop 后严格
/// 增长(证停机 flush 把末尾事件真落了盘,没被进程退出截断)。
#[test]
fn daemon_records_app_lifecycle_around_run() -> color_eyre::Result<()> {
    let mut daemon = Daemon::spawn_null("applifecycle")?;
    // 直读 stats.db(离线,不占 busy):poll 到 start 事件落库(events≥1)。
    let deadline = Instant::now() + Duration::from_secs(10);
    let running = loop {
        let out = daemon.cli_args_output(&["stats", "status"])?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if out.status.success()
            && let Some(n) = parse_events(&stdout)
            && n >= 1
        {
            break n;
        }
        if Instant::now() >= deadline {
            bail!(
                "app_lifecycle start 超时未落 stats.db,最后 stdout:\n{stdout}\nstderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // graceful 关停:daemon 记 app_lifecycle stop + flush,进程退出前必落库。
    daemon.sigterm()?;
    daemon.wait_for_exit()?;
    let out = daemon.cli_args_output(&["stats", "status"])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let after = parse_events(&stdout)
        .ok_or_else(|| color_eyre::eyre::eyre!("stop 后 status 无 events 行:\n{stdout}"))?;
    assert!(
        after > running,
        "graceful stop 应 flush 落 app_lifecycle stop(running={running} → after={after}):\n{stdout}"
    );
    Ok(())
}
