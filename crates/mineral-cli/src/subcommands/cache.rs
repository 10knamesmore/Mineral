//! `mineral cache` — 缓存管理子命令树。
//!
//! 目前仅 `clean`:清理所有可重建缓存(音频/封面 blob + 歌单缓存表),
//! 保留播放统计 / 喜欢 / 历史 / 会话 / song_meta,也不碰各 channel 的登录凭证。

use clap::Subcommand;
use color_eyre::eyre::WrapErr;

/// 缓存管理。
#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// 清理所有可重建缓存(音频/封面/歌单),保留播放统计 / 喜欢 / 历史。
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
        CacheCommand::Clean => clean().await,
    }
}

/// 执行 `cache clean`:清歌单缓存表 + 音频/封面 blob 目录。
///
/// # Return:
///   全部清理成功返回 `Ok(())`。某子项不存在(目录/库未创建)视为已清空,不报错。
async fn clean() -> color_eyre::Result<()> {
    // 1) 歌单缓存表(打开 db,清 playlist_cache/playlist_tracks;db 不存在则建空库)。
    // sqlite `mode=rwc` 只建文件不建父目录,fresh env 下需先确保 data_dir 存在。
    let data_dir = mineral_paths::data_dir()?;
    std::fs::create_dir_all(&data_dir)
        .wrap_err_with(|| format!("create data dir {}", data_dir.display()))?;
    let db_path = data_dir.join("mineral.db");
    let persist = mineral_persist::ServerStore::open(&db_path).await?;
    persist.clear_playlist_caches().await?;

    // 2) 音频缓存索引(`audio_cache` 表在 mineral.db;capacity 在此无意义,clear 不依赖它)。
    persist
        .audio_cache(mineral_paths::audio_cache_dir()?, /*capacity*/ 0)
        .await?
        .clear()
        .await?;

    // 3) 封面缓存索引(`cover_cache` 表在 client 的 tui.db;db 父目录即 data_dir,上面已建)。
    mineral_persist::ClientStore::open(&mineral_paths::tui_db()?)
        .await?
        .cover_cache(mineral_paths::cover_cache_dir()?, /*capacity*/ 0)
        .await?
        .clear()
        .await?;

    println!("已清理音频 / 封面 / 歌单缓存(播放统计、喜欢、历史已保留)");
    Ok(())
}
