//! `cache` 子命令的解析与执行:打开存储、取快照 / 清理,再交给 [`super::render`] 出文本。

use std::io::IsTerminal;
use std::time::SystemTime;

use clap::Subcommand;
use color_eyre::eyre::WrapErr;
use mineral_config::{AUDIO_CACHE_CAPACITY, COVER_CACHE_CAPACITY};
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
    // sqlite `mode=rwc` 只建文件不建父目录,fresh env 下需先确保 data_dir 存在。
    let data_dir = mineral_paths::data_dir()?;
    std::fs::create_dir_all(&data_dir)
        .wrap_err_with(|| format!("create data dir {}", data_dir.display()))?;

    let persist = ServerStore::open(&data_dir.join("mineral.db")).await?;
    let audio_stats = persist
        .audio_cache(mineral_paths::audio_cache_dir()?, AUDIO_CACHE_CAPACITY)
        .await?
        .snapshot();
    let audio = build_audio_input(audio_stats);
    let playlist = persist.playlist_cache_stats().await?;

    let cover_stats = ClientStore::open(&mineral_paths::tui_db()?)
        .await?
        .cover_cache(mineral_paths::cover_cache_dir()?, COVER_CACHE_CAPACITY)
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
