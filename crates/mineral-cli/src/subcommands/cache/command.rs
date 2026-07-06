//! `cache` 子命令的解析与执行:打开存储、取快照 / 清理,再交给 [`super::render`] 出文本。

use std::io::IsTerminal;
use std::time::SystemTime;

use clap::Subcommand;
use color_eyre::eyre::WrapErr;
use mineral_persist::{CacheStats, ClientStore, ServerStore};

use super::render::{self, AudioEntry, AudioInput, CoverInput};

/// 缓存管理。
#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// 展示音频 / 封面 / 歌单缓存的当前状态。
    Status {
        /// 展示逐条清单与按音质分布等更详细信息。
        #[arg(long)]
        detail: bool,
    },

    /// 清理音频 / 封面 / 歌单缓存(保留播放统计 / 喜欢 / 历史),并展示清理效果。
    Clean,

    /// 完全删除所缓存，包括db 与 cover/audio/playlists
    Reset {
        /// 确认执行;不带此 flag 只打印将删除的路径,不动盘。
        #[arg(long)]
        yes: bool,
    },
}

/// 按 [`CacheCommand`] 分发到具体实现。
///
/// # Params:
///   - `command`: 已解析的 cache 子命令。
///
/// # Return:
///   命令执行结果。
pub async fn run(command: CacheCommand) -> color_eyre::Result<()> {
    match command {
        CacheCommand::Status { detail } => status(detail).await,
        CacheCommand::Clean => clean().await,
        CacheCommand::Reset { yes } => reset(yes),
    }
}

/// `cache status`:取三类缓存快照,渲染状态报告。
///
/// # Params:
///   - `detail`: 是否展示逐条清单与按音质分布。
///
/// # Return:
///   渲染并打印成功返回 `Ok(())`。
async fn status(detail: bool) -> color_eyre::Result<()> {
    // CLI 离线自 eval 配置取容量(与 daemon 同一真相源);用户配置坏已在 loader 降级默认。
    let (config, _warnings) =
        mineral_config::load(&mineral_paths::config_dir()?.join("config.lua"))?;
    // sqlite `mode=rwc` 只建文件不建父目录,fresh env 下需先确保 data_dir 存在。
    let data_dir = mineral_paths::data_dir()?;
    std::fs::create_dir_all(&data_dir)
        .wrap_err_with(|| format!("create data dir {}", data_dir.display()))?;

    let persist = ServerStore::open(&data_dir.join("mineral.db")).await?;
    let audio_stats = persist
        .audio_cache(
            mineral_paths::audio_cache_dir()?,
            *config.cache().audio_capacity(),
        )
        .await?
        .snapshot();
    let audio = build_audio_input(audio_stats);
    let playlist = persist.playlist_cache_stats().await?;

    let cover_stats = ClientStore::open(&mineral_paths::tui_db()?)
        .await?
        .cover_cache(
            mineral_paths::cover_cache_dir()?,
            *config.cache().cover_capacity(),
        )
        .await?
        .snapshot();
    let cover = CoverInput {
        count: cover_stats.entries.len(),
        total_bytes: cover_stats.total_bytes,
        capacity: cover_stats.capacity,
    };

    let color = std::io::stdout().is_terminal();
    println!(
        "{}",
        render::render_status(&audio, &cover, &playlist, detail, color, SystemTime::now())
    );
    Ok(())
}

/// `cache clean`:清三类缓存,各取清理回执,渲染前后对比。
///
/// # Return:
///   全部清理成功返回 `Ok(())`。某子项不存在(目录 / 库未创建)视为已清空,不报错。
async fn clean() -> color_eyre::Result<()> {
    let data_dir = mineral_paths::data_dir()?;
    std::fs::create_dir_all(&data_dir)
        .wrap_err_with(|| format!("create data dir {}", data_dir.display()))?;

    let persist = ServerStore::open(&data_dir.join("mineral.db")).await?;
    let playlist = persist.clear_playlist_caches().await?;
    // capacity 在 clear 时无意义,传 0。
    let audio = persist
        .audio_cache(mineral_paths::audio_cache_dir()?, /*capacity*/ 0)
        .await?
        .clear()
        .await?;
    let cover = ClientStore::open(&mineral_paths::tui_db()?)
        .await?
        .cover_cache(mineral_paths::cover_cache_dir()?, /*capacity*/ 0)
        .await?
        .clear()
        .await?;

    let color = std::io::stdout().is_terminal();
    println!("{}", render::render_clean(&audio, &cover, &playlist, color));
    Ok(())
}

/// `cache reset`:收集两个库文件(含 sqlite WAL 伴生文件)与两个缓存目录,
/// 无 `--yes` 只打印删除计划,带 `--yes` 逐一删除(路径不存在视为已删,不报错)。
///
/// # Params:
///   - `yes`: 是否真正执行删除。
///
/// # Return:
///   打印计划 / 回执后返回 `Ok(())`;删除失败(如 daemon 仍占用库文件)冒泡报错。
fn reset(yes: bool) -> color_eyre::Result<()> {
    let server_db = mineral_paths::data_dir()?.join("mineral.db");
    let client_db = mineral_paths::tui_db()?;
    let mut rows = Vec::<render::ResetRow>::new();

    for db in [server_db, client_db] {
        // sqlite WAL 模式的 -wal / -shm 伴生文件必须与主库同删:半套残留会让重建的库
        // 在下次打开时读到旧页,比不删更糟。
        for suffix in ["", "-wal", "-shm"] {
            let mut os = db.as_os_str().to_owned();
            os.push(suffix);
            rows.push(reset_file(&std::path::PathBuf::from(os), yes)?);
        }
    }
    for dir in [
        mineral_paths::audio_cache_dir()?,
        mineral_paths::cover_cache_dir()?,
    ] {
        rows.push(reset_dir(&dir, yes)?);
    }

    let color = std::io::stdout().is_terminal();
    println!("{}", render::render_reset(&rows, /*executed*/ yes, color));
    Ok(())
}

/// 删除(或计划删除)单个库文件,产出渲染行。不存在 → 「不存在」。
fn reset_file(path: &std::path::Path, yes: bool) -> color_eyre::Result<render::ResetRow> {
    let outcome = if !yes {
        "将删除"
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => "已删除",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "不存在",
            Err(e) => {
                return Err(e).wrap_err_with(|| format!("删除 {} 失败", path.display()));
            }
        }
    };
    Ok(render::ResetRow {
        path: path.display().to_string(),
        kind: "库文件",
        outcome,
    })
}

/// 删除(或计划删除)单个缓存目录,产出渲染行。不存在 → 「不存在」。
fn reset_dir(path: &std::path::Path, yes: bool) -> color_eyre::Result<render::ResetRow> {
    let outcome = if !yes {
        "将删除"
    } else {
        match std::fs::remove_dir_all(path) {
            Ok(()) => "已删除",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "不存在",
            Err(e) => {
                return Err(e).wrap_err_with(|| format!("删除 {} 失败", path.display()));
            }
        }
    };
    Ok(render::ResetRow {
        path: path.display().to_string(),
        kind: "缓存目录",
        outcome,
    })
}

/// 把音频缓存快照转成渲染输入:逐条 stat 文件 mtime(供「最旧 / 最新」与 detail 用)。
///
/// # Params:
///   - `stats`: 音频缓存只读快照(含 `root` 与各条 relpath)。
///
/// # Return:
///   带 mtime 的渲染输入;stat 失败的条目 mtime 记为 `None`(不致命)。
fn build_audio_input(stats: CacheStats) -> AudioInput {
    let root = stats.root;
    let entries = stats
        .entries
        .into_iter()
        .map(|e| {
            let mtime = root.as_ref().and_then(|r| {
                std::fs::metadata(r.join(&e.relpath))
                    .and_then(|m| m.modified())
                    .ok()
            });
            AudioEntry {
                relpath: e.relpath,
                bytes: e.bytes,
                mtime,
            }
        })
        .collect::<Vec<_>>();
    AudioInput {
        entries,
        total_bytes: stats.total_bytes,
        capacity: stats.capacity,
    }
}
