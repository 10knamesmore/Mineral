//! 接收落盘歌曲的打标任务，按歌曲、音质和文件路径去重并启动 worker 池。
//!
//! 投递不等待采集和写盘；关闭开关时投递恒为 no-op。完成后移除去重标记，允许重投。

use std::path::PathBuf;
use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, Song, SongId};
use parking_lot::Mutex;
use rustc_hash::FxHashSet;

use super::assemble;
use super::worker::run;
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
}

/// 打标去重键:`{song_id.qualified()}:{quality}:{path}`。
/// 键含路径:同曲同音质的导出文件与缓存文件是两份实体,可能几乎同时落盘,都要打。
///
/// # Params:
///   - `song_id`: 歌曲 ID
///   - `quality`: 入库音质
///   - `path`: 落盘文件路径
///
/// # Return:
///   去重键字符串。
pub(super) fn job_key(
    song_id: &mineral_model::SongId,
    quality: BitRate,
    path: &std::path::Path,
) -> String {
    format!("{}:{}", cache_key(song_id, quality), path.display())
}

/// 队列内部(投递端与 worker 共享)。
pub(super) struct QueueInner {
    /// 任务入队端。
    tx: tokio::sync::mpsc::UnboundedSender<TagJob>,

    /// 在队 / 在写集合(去重键 = 缓存索引键);worker 消费完剔除,允许之后再投。
    pub(super) inflight: Arc<Mutex<FxHashSet<String>>>,
}

/// 打标队列投递句柄。开关关闭时是 null-object:`enqueue` 恒 no-op。
#[derive(Clone)]
pub(crate) struct TaggingQueue {
    /// `None` = 打标关闭(配置 `download.tagging = false`)。
    pub(super) inner: Option<Arc<QueueInner>>,
}

impl TaggingQueue {
    /// 起打标 worker 池并返回投递句柄。
    ///
    /// # Params:
    ///   - `enabled`: 配置开关(`download.tagging`);`false` 返回 null-object
    ///   - `channels`: 已注入的 channel(worker 按歌曲来源路由采集;按需 clone)
    ///   - `http`: 下载用 HTTP client(GET 封面;`None` 时封面字段缺省)
    ///   - `workers`: 并发 worker 数(配置 `download.tagging_workers`;`<1` 按 1)
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
            return Self { inner: None };
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // mpsc 单消费者:多 worker 共享一个 receiver(锁只护 recv,不跨任务处理)。
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let inflight = Arc::new(Mutex::new(FxHashSet::default()));
        let album_cache = assemble::AlbumCache::default();
        for _ in 0..workers.max(1) {
            tokio::spawn(run(
                Arc::clone(&rx),
                channels.to_vec(),
                http.cloned(),
                Arc::clone(&inflight),
                album_cache.clone(),
            ));
        }
        Self {
            inner: Some(Arc::new(QueueInner { tx, inflight })),
        }
    }

    /// 投递一条打标任务;同一文件(同曲 + 同音质 + 同路径)在队 / 在写时去重。
    ///
    /// # Params:
    ///   - `song`: 在库歌曲
    ///   - `path`: 落盘文件路径
    ///   - `quality`: 入库音质
    ///
    /// # Return:
    ///   `true` = 已受理(未去重);`false` = 去重丢弃 / 开关关闭。
    pub(crate) fn enqueue(&self, song: Song, path: PathBuf, quality: BitRate) -> bool {
        let key = job_key(&song.id, quality, &path);
        self.send(
            TagJob {
                song,
                path,
                quality,
            },
            &key,
        )
    }

    /// 去重 + 发送(null-object 恒 no-op;发送失败还原去重标记)。
    ///
    /// # Params:
    ///   - `job`: 任务
    ///   - `key`: 去重键
    ///
    /// # Return:
    ///   `true` = 已受理(计数 +1);`false` = 去重丢弃 / 开关关闭 / 队列已关。
    fn send(&self, job: TagJob, key: &str) -> bool {
        let Some(inner) = &self.inner else {
            return false;
        };
        if !inner.inflight.lock().insert(key.to_owned()) {
            return false;
        }
        if inner.tx.send(job).is_err() {
            // worker 已随 server 关闭:还原去重标记(下次进程再投)。
            inner.inflight.lock().remove(key);
            return false;
        }
        true
    }
}
