//! 处理单首打标任务，按歌曲来源采集专辑、歌词和封面，再在阻塞线程写入文件。
//!
//! 单首失败只记日志；任务排队与去重由调度任务负责。

use std::sync::Arc;

use mineral_channel_core::MusicChannel;

use super::assemble;
use super::queue::TagJob;
use super::write;

/// 打标一首:三路并发采集 → `spawn_blocking` 写盘。
///
/// # Params:
///   - `job`: 打标任务
///   - `channels`: 已注入的 channel
///   - `http`: 下载用 HTTP client
///   - `album_cache`: 专辑详情缓存
pub(super) async fn process(
    job: &TagJob,
    channels: &[Arc<dyn MusicChannel>],
    http: Option<&reqwest::Client>,
    album_cache: &assemble::AlbumCache,
) {
    let Some(channel) = channels
        .iter()
        .find(|ch| ch.source() == job.song_id().namespace())
    else {
        mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), "无对应 channel,跳过打标");
        return;
    };
    let song = &job.song;
    let tags = assemble::collect(channel.as_ref(), http, song, album_cache).await;
    let path = job.path.clone();
    let result = tokio::task::spawn_blocking(move || write::write_tags(&path, &tags)).await;
    match result {
        Ok(Ok(write::WriteOutcome::Tagged)) => {
            mineral_log::info!(target: "tagging", song_id = job.song_id().as_str(), path = %job.path.display(), "已写入内嵌 tag");
        }
        Ok(Ok(write::WriteOutcome::SkippedUnsupported)) => {
            mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), path = %job.path.display(), "内容无法探测或容器不支持,跳过打标");
        }
        Ok(Err(e)) => {
            mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), error = mineral_log::chain(&e), "打标失败");
        }
        Err(e) => {
            mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), error = mineral_log::chain(&e), "打标任务被取消");
        }
    }
}
