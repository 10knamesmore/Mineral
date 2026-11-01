//! 接收落盘歌曲的打标任务，由调度任务独占去重、排队和完成处理。
//!
//! 投递不等待采集和写盘；关闭开关时投递恒为 no-op。完成后移除去重标记，允许重投。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, Song, SongId};
use rustc_hash::FxHashSet;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::assemble;
use super::worker::process;
use crate::media_cache::cache_key;

/// 一条打标任务(落盘文件 + 其歌曲)。
pub(super) struct TagJob {
    /// 待打标的歌曲(元数据已就绪)。
    pub(super) song: Song,

    /// 落盘文件(导出或缓存库路径)。
    pub(super) path: PathBuf,

    /// 入库音质(去重键维度之一)。
    pub(super) quality: BitRate,
}

impl TagJob {
    /// 歌曲 id(去重键 / 日志用)。
    pub(super) fn song_id(&self) -> &SongId {
        &self.song.id
    }

    /// 路径参与去重，同曲同音质的导出文件与缓存文件分别打标。
    fn key(&self) -> String {
        format!(
            "{}:{}",
            cache_key(&self.song.id, self.quality),
            self.path.display()
        )
    }
}

/// 打标队列投递句柄。开关关闭时是 null-object:`enqueue` 恒 no-op。
#[derive(Clone)]
pub(crate) struct TaggingQueue {
    /// `None` = 打标关闭(配置 `download.tagging = false`)。
    tx: Option<UnboundedSender<TagJob>>,
}

impl TaggingQueue {
    /// 启动打标调度任务并返回投递句柄。
    ///
    /// # Params:
    ///   - `enabled`: 配置开关(`download.tagging`);`false` 返回 null-object
    ///   - `channels`: 已注入的 channel，按歌曲来源路由采集
    ///   - `http`: 下载用 HTTP client(GET 封面;`None` 时封面字段缺省)
    ///   - `workers`: 同时处理的任务数(配置 `download.tagging_workers`;`<1` 按 1)
    ///
    /// # Return:
    ///   投递句柄(廉价 clone)。
    pub(crate) fn spawn(
        enabled: bool,
        channels: &[Arc<dyn MusicChannel>],
        http: Option<&reqwest::Client>,
        workers: usize,
    ) -> Self {
        if !enabled {
            return Self { tx: None };
        }
        let (tx, rx) = unbounded_channel();
        tokio::spawn(run_tagging(
            rx,
            channels.to_vec(),
            http.cloned(),
            workers.max(1),
        ));
        Self { tx: Some(tx) }
    }

    /// 投递后立即返回；调度任务接收时，丢弃同曲、同音质、同路径的未完成重复任务。
    /// 前一次完成后才接收到的请求会重新打标。关闭开关或接收端退出时忽略投递。
    pub(crate) fn enqueue(&self, song: Song, path: PathBuf, quality: BitRate) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(TagJob {
                song,
                path,
                quality,
            });
        }
    }
}

/// 独占等待队列与未完成键；所有投递句柄释放后，处理完剩余任务再退出。
async fn run_tagging(
    mut rx: UnboundedReceiver<TagJob>,
    channels: Vec<Arc<dyn MusicChannel>>,
    http: Option<reqwest::Client>,
    concurrency: usize,
) {
    let album_cache = assemble::AlbumCache::default();
    let channels = channels.as_slice();
    let http = http.as_ref();
    let album_cache = &album_cache;
    let mut pending = VecDeque::new();
    let mut unfinished = FxHashSet::default();
    let mut running = FuturesUnordered::new();
    let mut receiving = true;

    while receiving || !pending.is_empty() || !running.is_empty() {
        while running.len() < concurrency {
            let Some((key, job)) = pending.pop_front() else {
                break;
            };
            running.push(async move {
                process(&job, channels, http, album_cache).await;
                key
            });
        }

        tokio::select! {
            job = rx.recv(), if receiving => {
                match job {
                    Some(job) => {
                        let key = job.key();
                        if unfinished.insert(key.clone()) {
                            pending.push_back((key, job));
                        }
                    }
                    None => receiving = false,
                }
            }
            Some(key) = running.next(), if !running.is_empty() => {
                unfinished.remove(&key);
            }
        }
    }
}
