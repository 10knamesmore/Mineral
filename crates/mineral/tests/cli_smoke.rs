//! CLI 冒烟 e2e:用 `assert_cmd` 起真 `mineral` 二进制,验证 help / version 退出码、
//! 错误参数非零退出、`mineral status` 无 daemon 时友好报错。
//!
//! 不需要 pty / 网络;每个用例隔离一套临时 XDG 目录(保证 status 连不上真 daemon)。

use assert_cmd::Command;
use color_eyre::eyre::WrapErr;
use predicates::str::contains;

/// 纳秒时间戳,给隔离目录名做唯一后缀。
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 构造一条隔离 XDG 环境的 `mineral` 命令(runtime 指向空临时目录 → status 必连不上)。
///
/// socket 不走 XDG 隔离目录:macOS 的 `temp_dir()` 在 `/var/folders/<xx>/<长哈希>/T/` 下,
/// 叠加唯一后缀后 `<runtime>/mineral/mineral.sock` 会顶破 AF_UNIX `sun_path` 上限
/// (104 字节),status 报「路径过长」而非预期行为。改经 `$MINERAL_SOCKET_DIR` 指到
/// `/tmp` 下的短路径——同样每用例唯一,「连不上真 daemon」的隔离保证不变。
fn mineral() -> color_eyre::Result<Command> {
    let suffix = format!("{}-{}", std::process::id(), unique_suffix());
    let root = std::env::temp_dir().join(format!("mineral-cli-{suffix}"));
    let mut cmd = Command::cargo_bin("mineral").wrap_err("locate mineral binary")?;
    cmd.env("MINERAL_SOCKET_DIR", format!("/tmp/mineral-cli-{suffix}"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"));
    Ok(cmd)
}

/// `--help` / `--version` 退出码 0。
#[test]
fn help_and_version_exit_zero() -> color_eyre::Result<()> {
    mineral()?.arg("--help").assert().success();
    mineral()?.arg("--version").assert().success();
    Ok(())
}

/// 未知子命令 / 未知 flag → clap 报错、非零退出。
#[test]
fn bad_args_exit_nonzero() -> color_eyre::Result<()> {
    mineral()?
        .arg("definitely-not-a-command")
        .assert()
        .failure();
    mineral()?.arg("--definitely-not-a-flag").assert().failure();
    Ok(())
}

/// `mineral cache clean` 在全新隔离环境(无 db / 无缓存目录)下也能优雅成功并退出 0,
/// 并打出「前后对比 + 总释放」报告。
#[test]
fn cache_clean_succeeds_on_fresh_env() -> color_eyre::Result<()> {
    mineral()?
        .args(["cache", "clean"])
        .assert()
        .success()
        .stdout(contains("已清空"))
        .stdout(contains("共释放"));
    Ok(())
}

/// `mineral cache status` 在全新隔离环境下也能优雅成功并退出 0,打出三区域汇总。
#[test]
fn cache_status_succeeds_on_fresh_env() -> color_eyre::Result<()> {
    mineral()?
        .args(["cache", "status"])
        .assert()
        .success()
        .stdout(contains("音频"))
        .stdout(contains("歌单"));
    Ok(())
}

/// `mineral status` 无 daemon → 非零退出,且 stderr 给出「先跑 mineral serve」的提示。
#[test]
fn status_without_daemon_errors_with_hint() -> color_eyre::Result<()> {
    mineral()?
        .arg("status")
        .assert()
        .failure()
        .stderr(contains("serve"));
    Ok(())
}
