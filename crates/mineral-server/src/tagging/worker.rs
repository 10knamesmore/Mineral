//! 消费打标任务，按歌曲来源采集专辑、歌词和封面，再在阻塞线程写入文件。
//!
//! 多个 worker 共享接收端和专辑缓存；单首失败只记日志，完成后释放去重标记。

use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use parking_lot::Mutex;
use rustc_hash::FxHashSet;

use super::assemble;
use super::queue::{TagJob, job_key};
use super::write;

/// worker 主循环:从共享 receiver 取任务,逐首采集 + 写盘;单首的任何失败只记日志。
///
/// # Params:
///   - `rx`: 共享任务接收端(多 worker 抢单,锁只护 `recv` 调用本身)
///   - `channels`: 已注入的 channel(按歌曲来源路由)
///   - `http`: 下载用 HTTP client(GET 封面)
///   - `inflight`: 在队 / 在写集合(消费完剔除)
///   - `album_cache`: 专辑详情缓存(worker 池共享)
pub(super) async fn run(
    rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<TagJob>>>,
    channels: Vec<Arc<dyn MusicChannel>>,
    http: Option<reqwest::Client>,
    inflight: Arc<Mutex<FxHashSet<String>>>,
    album_cache: assemble::AlbumCache,
) {
    loop {
        // 锁的临时 guard 在本语句结束即释放,不跨任务处理。
        let job = rx.lock().await.recv().await;
        let Some(job) = job else { break };
        process(&job, &channels, http.as_ref(), &album_cache).await;
        inflight
            .lock()
            .remove(&job_key(job.song_id(), job.quality, &job.path));
    }
}

/// 打标一首:三路并发采集 → `spawn_blocking` 写盘。
///
/// # Params:
///   - `job`: 打标任务
///   - `channels`: 已注入的 channel
///   - `http`: 下载用 HTTP client
///   - `album_cache`: 专辑详情缓存
async fn process(
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
