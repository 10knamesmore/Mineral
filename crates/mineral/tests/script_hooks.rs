//! daemon 进程级 e2e:用户 `config.lua` 的脚本钩子在真 daemon 内生效。
//!
//! 起真 `mineral serve` 子进程(预埋 config.lua),经 `mineral action <name>`
//! 子命令穿一整条链:CLI → unix socket → daemon dispatch → 脚本线程查注册表
//! → Lua 回调 → 结果回包。音频走 `MINERAL_AUDIO_NULL` 降级,headless 稳跑。
//!
//! 与 daemon_lifecycle 同进 nextest 的 `daemon-e2e` 串行组(真子进程 + socket)。

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, bail};

/// 隔离环境里的一个 daemon 子进程;Drop 时 kill 子进程并清临时目录。
struct Daemon {
    /// `mineral serve` 子进程。
    child: Child,

    /// 隔离用的临时根目录(XDG_CONFIG/DATA/CACHE 全指到这下面)。
    root: PathBuf,

    /// socket 目录(经 `MINERAL_SOCKET_DIR` 注入,刻意短于 `root`,
    /// 压在 `sun_path` 上限内)。
    sock_dir: PathBuf,

    /// daemon 监听的 socket 路径。
    socket: PathBuf,
}

impl Daemon {
    /// 起一个隔离环境、null 音频后端的 daemon;`config_lua` 为 `Some` 时
    /// 预埋成用户 config.lua(脚本钩子的输入)。
    fn spawn(tag: &str, config_lua: Option<&str>) -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-script-e2e-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        let sock_dir =
            std::env::temp_dir().join(format!("mnls-{}-{}", std::process::id(), unique_suffix()));
        std::fs::create_dir_all(&root).wrap_err("create isolated root dir")?;
        if let Some(src) = config_lua {
            let cfg_dir = root.join("config/mineral");
            std::fs::create_dir_all(&cfg_dir).wrap_err("create config dir")?;
            std::fs::write(cfg_dir.join("config.lua"), src).wrap_err("seed config.lua")?;
        }
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mineral"));
        cmd.arg("serve")
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("MINERAL_SOCKET_DIR", &sock_dir)
            .env("MINERAL_AUDIO_NULL", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().wrap_err("spawn `mineral serve`")?;
        let socket = sock_dir.join("mineral.sock");
        Ok(Self {
            child,
            root,
            sock_dir,
            socket,
        })
    }

    /// 轮询直到 socket 可连(daemon ready),超时则报错。
    fn wait_ready(&self) -> color_eyre::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if UnixStream::connect(&self.socket).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("daemon did not become ready in time")
    }

    /// 在同一隔离环境下跑 `mineral action <name>`,捕获输出。
    fn action_output(&self, name: &str) -> color_eyre::Result<std::process::Output> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mineral"));
        cmd.arg("action")
            .arg(name)
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("MINERAL_SOCKET_DIR", &self.sock_dir)
            .stdin(Stdio::null());
        cmd.output().wrap_err("run `mineral action`")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.sock_dir);
    }
}

/// 纳秒时间戳,给临时目录名做唯一后缀。
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 注册过的动作经 CLI 触发成功;未注册的动作回人读错误;脚本回调里的
/// Lua 错误经回执变成非零退出。一只 daemon 串三个断言(进程级 e2e 起停贵)。
#[test]
fn registered_action_runs_and_failures_surface() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn(
        "action",
        Some(
            r#"
            mineral.action("e2e.echo", function(ctx) mineral.log.info("echoed") end)
            mineral.action("e2e.boom", function(ctx) error("kapow") end)
            return {}
            "#,
        ),
    )?;
    daemon.wait_ready()?;

    let ok = daemon.action_output("e2e.echo")?;
    assert!(
        ok.status.success(),
        "已注册动作应成功,stderr: {}",
        String::from_utf8_lossy(&ok.stderr)
    );

    let missing = daemon.action_output("e2e.nope")?;
    assert!(!missing.status.success(), "未注册动作必须非零退出");
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("未注册"), "错误应说明未注册,实得: {stderr}");

    let failing = daemon.action_output("e2e.boom")?;
    assert!(!failing.status.success(), "回调出错必须非零退出");
    let stderr = String::from_utf8_lossy(&failing.stderr);
    assert!(
        stderr.contains("kapow"),
        "错误应带回调失败信息,实得: {stderr}"
    );
    Ok(())
}

/// 无 config.lua 的 daemon:脚本未启用,触发任何动作都报人读错误。
#[test]
fn action_without_script_reports_disabled() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("noscript", /*config_lua*/ None)?;
    daemon.wait_ready()?;
    let out = daemon.action_output("whatever")?;
    assert!(!out.status.success(), "无脚本必须非零退出");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("脚本未启用"),
        "错误应说明脚本未启用,实得: {stderr}"
    );
    Ok(())
}
