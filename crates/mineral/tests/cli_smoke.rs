//! CLI 冒烟 e2e:用 `assert_cmd` 起真 `mineral` 二进制,验证 help / version 退出码、
//! 错误参数非零退出、`mineral status` 无 daemon 时友好报错、`stats` 离线子命令。
//!
//! 不需要 pty / 网络;每个用例隔离一套临时 XDG 目录(保证 status 连不上真 daemon)。

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use color_eyre::eyre::WrapErr;
use mineral_stats::StatsStore;
use predicates::str::contains;

/// 纳秒时间戳,给隔离目录名做唯一后缀。
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 每用例唯一的隔离根(纳秒 + pid 后缀)。同一根可配对「先造 db、再跑二进制」。
fn unique_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "mineral-cli-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
}

/// 在给定隔离根下构造一条 `mineral` 命令(XDG 全指向该根,socket 走 `/tmp` 短路径)。
///
/// socket 不走 XDG 隔离目录:macOS 的 `temp_dir()` 在 `/var/folders/<xx>/<长哈希>/T/` 下,
/// 叠加唯一后缀后 `<runtime>/mineral/mineral.sock` 会顶破 AF_UNIX `sun_path` 上限
/// (104 字节),status 报「路径过长」而非预期行为。改经 `$MINERAL_SOCKET_DIR` 指到
/// `/tmp` 下的短路径——同样每用例唯一,「连不上真 daemon」的隔离保证不变。
fn mineral_at(root: &Path) -> color_eyre::Result<Command> {
    let short = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("mineral-cli");
    let mut cmd = Command::cargo_bin("mineral").wrap_err("locate mineral binary")?;
    cmd.env("MINERAL_SOCKET_DIR", format!("/tmp/{short}"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"));
    Ok(cmd)
}

/// 构造一条隔离 XDG 环境的 `mineral` 命令(每次全新根;status 必连不上真 daemon)。
fn mineral() -> color_eyre::Result<Command> {
    mineral_at(&unique_root())
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

/// 全新隔离环境(无 stats.db)下,`stats status` 优雅成功并指向 `stats.level` 配置。
#[test]
fn stats_absent_db_points_to_config() -> color_eyre::Result<()> {
    mineral()?
        .args(["stats", "status"])
        .assert()
        .success()
        .stdout(contains("stats.level"));
    Ok(())
}

/// 时间窗 / 格式 / 榜类别的参数面在真二进制上解析通过;缺库时各命令优雅退 0。
#[test]
fn stats_arg_surface_parses() -> color_eyre::Result<()> {
    // 窗口 --year + 格式 md。
    mineral()?
        .args(["stats", "report", "--year", "2026", "--format", "md"])
        .assert()
        .success();
    // 榜类别 + --all + json。
    mineral()?
        .args(["stats", "top", "songs", "--all", "--format", "json"])
        .assert()
        .success();
    // 非法榜类别 → clap 非零退出。
    mineral()?
        .args(["stats", "top", "notacategory"])
        .assert()
        .failure();
    // prune 缺 --before(必填)→ 非零退出。
    mineral()?.args(["stats", "prune"]).assert().failure();
    Ok(())
}

/// 造一个含数据的 stats.db,验证 `report --format json` 出结构化全量、`prune` dry-run 出计划。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stats_report_json_and_prune_plan_over_seeded_db() -> color_eyre::Result<()> {
    let root = unique_root();
    let stats_db = root.join("data/mineral/stats.db");
    let parent = stats_db
        .parent()
        .ok_or_else(|| color_eyre::eyre::eyre!("stats.db 无父目录"))?;
    std::fs::create_dir_all(parent).wrap_err("建 data 目录")?;
    {
        // 种 5 行播放 + 3 条事件,落盘后关句柄(释放连接;WAL 下子进程照样读得到已提交行)。
        let store = StatsStore::open(&stats_db).await?;
        mineral_stats::fixture::seed(&store, /*plays*/ 5, /*events*/ 3).await?;
    }

    // report --all --format json:结构化全量,含总量分节与其字段。
    mineral_at(&root)?
        .args(["stats", "report", "--all", "--format", "json"])
        .assert()
        .success()
        .stdout(contains("\"totals\""))
        .stdout(contains("\"plays\""));

    // prune --before 远未来 无 --yes:只出将删行数计划,不动盘。
    mineral_at(&root)?
        .args(["stats", "prune", "--before", "2099-01-01"])
        .assert()
        .success()
        .stdout(contains("将删除"));

    // 计划态未动盘:db 里仍是 5 行 plays。
    let store = StatsStore::open(&stats_db).await?;
    assert_eq!(
        store.recent_plays(0..i64::MAX, None, 100).await?.len(),
        5,
        "dry-run 不删行"
    );
    Ok(())
}
