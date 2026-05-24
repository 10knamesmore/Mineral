//! CLI 冒烟 e2e:用真 `mineral` 二进制(`CARGO_BIN_EXE_mineral`)验证 help / version
//! 退出码、错误参数非零退出、`mineral status` 无 daemon 时友好报错。
//!
//! 不需要 pty / 网络;每个用例隔离一套临时 XDG 目录(保证 status 连不上真 daemon)。

use std::process::Command;

use color_eyre::eyre::WrapErr;

/// 纳秒时间戳,给隔离目录名做唯一后缀。
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 构造一条隔离 XDG 环境的 `mineral` 命令(runtime 指向空临时目录 → status 必连不上)。
fn mineral() -> Command {
    let root = std::env::temp_dir().join(format!(
        "mineral-cli-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mineral"));
    cmd.env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"));
    cmd
}

/// `--help` / `--version` 退出码 0。
#[test]
fn help_and_version_exit_zero() -> color_eyre::Result<()> {
    let help = mineral().arg("--help").output().wrap_err("run --help")?;
    assert!(help.status.success(), "--help 应退出 0");
    let version = mineral()
        .arg("--version")
        .output()
        .wrap_err("run --version")?;
    assert!(version.status.success(), "--version 应退出 0");
    Ok(())
}

/// 未知子命令 / 未知 flag → clap 报错、非零退出。
#[test]
fn bad_args_exit_nonzero() -> color_eyre::Result<()> {
    let unknown_cmd = mineral()
        .arg("definitely-not-a-command")
        .output()
        .wrap_err("run unknown subcommand")?;
    assert!(!unknown_cmd.status.success(), "未知子命令应非零退出");

    let bad_flag = mineral()
        .arg("--definitely-not-a-flag")
        .output()
        .wrap_err("run bad flag")?;
    assert!(!bad_flag.status.success(), "未知 flag 应非零退出");
    Ok(())
}

/// `mineral status` 无 daemon → 非零退出,且 stderr 给出「先跑 mineral serve」的提示。
#[test]
fn status_without_daemon_errors_with_hint() -> color_eyre::Result<()> {
    let out = mineral().arg("status").output().wrap_err("run status")?;
    assert!(!out.status.success(), "无 daemon 时 status 应非零退出");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("serve"),
        "stderr 应提示先 `mineral serve`,实际:\n{stderr}"
    );
    Ok(())
}
