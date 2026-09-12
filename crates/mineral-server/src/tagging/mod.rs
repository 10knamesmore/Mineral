//! 落盘歌曲的内嵌 metadata 打标:side 子系统,下载 / 缓存落盘后异步写 tag。
//!
//! 投递方只发消息不等待;N 个并发 worker(配置 `download.tagging_workers`)共享一个
//! mpsc receiver 抢单,按 `{song_id.qualified()}:{quality}:{path}` 去重在队 / 在写
//! 任务。单曲采集(专辑详情 / 歌词 / 封面)三路并发,专辑详情按专辑缓存(同一张专辑
//! 多首只拉一次)。失败只记日志——不影响下载与播放。

mod assemble;
mod write;

use std::path::PathBuf;
use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, Song, SongId};
use parking_lot::Mutex;
use rustc_hash::FxHashSet;

use crate::media_cache::cache_key;

/// 一条打标任务(落盘文件 + 其歌曲)。
struct TagJob {
    /// 待打标的歌曲(元数据已就绪)。
    song: Song,

    /// 落盘文件(导出或缓存库路径)。
    path: PathBuf,

    /// 入库音质(去重键维度之一)。
    quality: BitRate,
}

impl TagJob {
    /// 歌曲 id(去重键 / 日志用)。
    fn song_id(&self) -> &SongId {
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
fn job_key(song_id: &mineral_model::SongId, quality: BitRate, path: &std::path::Path) -> String {
    format!("{}:{}", cache_key(song_id, quality), path.display())
}

/// 队列内部(投递端与 worker 共享)。
struct QueueInner {
    /// 任务入队端。
    tx: tokio::sync::mpsc::UnboundedSender<TagJob>,

    /// 在队 / 在写集合(去重键 = 缓存索引键);worker 消费完剔除,允许之后再投。
    inflight: Arc<Mutex<FxHashSet<String>>>,
}

/// 打标队列投递句柄。开关关闭时是 null-object:`enqueue` 恒 no-op。
#[derive(Clone)]
pub(crate) struct TaggingQueue {
    /// `None` = 打标关闭(配置 `download.tagging = false`)。
    inner: Option<Arc<QueueInner>>,
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

/// worker 主循环:从共享 receiver 取任务,逐首采集 + 写盘;单首的任何失败只记日志。
///
/// # Params:
///   - `rx`: 共享任务接收端(多 worker 抢单,锁只护 `recv` 调用本身)
///   - `channels`: 已注入的 channel(按歌曲来源路由)
///   - `http`: 下载用 HTTP client(GET 封面)
///   - `inflight`: 在队 / 在写集合(消费完剔除)
///   - `album_cache`: 专辑详情缓存(worker 池共享)
async fn run(
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use lofty::file::TaggedFileExt as _;
    use lofty::tag::Accessor as _;
    use mineral_model::{AlbumRef, ArtistRef, SongId, SourceKind};
    use mineral_test::mock::{CannedChannel, serve_once};

    use super::*;

    /// 等待 `cond` 在 deadline 内成立(打标是后台串行任务,断言需轮询)。
    ///
    /// # Params:
    ///   - `cond`: 谓词
    ///   - `deadline`: 最长等待
    async fn wait_until(mut cond: impl FnMut() -> bool, deadline: Duration) {
        let start = Instant::now();
        while !cond() {
            assert!(start.elapsed() <= deadline, "等待超时({deadline:?})");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 探测文件是否已有带标题的 primary tag(轮询谓词共用)。
    fn file_has_title(path: &std::path::Path) -> bool {
        std::fs::File::open(path)
            .ok()
            .and_then(|mut f| {
                lofty::probe::Probe::new(&mut f)
                    .guess_file_type()
                    .ok()
                    .and_then(|p| p.read().ok())
            })
            .and_then(|t| t.primary_tag().map(|tag| tag.title().is_some()))
            .unwrap_or(false)
    }

    /// 测试歌曲(带专辑引用,封面 URL 可配)。
    fn song(id: &str, cover: Option<url::Url>) -> Song {
        Song::builder()
            .id(SongId::new(SourceKind::NETEASE, id))
            .name("晴天".to_owned())
            .artists(vec![ArtistRef {
                id: mineral_model::ArtistId::new(SourceKind::NETEASE, "6452"),
                name: "周杰伦".to_owned(),
            }])
            .album(Some(AlbumRef {
                id: mineral_model::AlbumId::new(SourceKind::NETEASE, "31655"),
                name: "叶惠美".to_owned(),
            }))
            .cover_url(cover.map(mineral_model::MediaUrl::Remote))
            .build()
    }

    /// 端到端:投递 → worker 采集 + 写盘 → 文件读回 tag。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enqueue_tags_file_end_to_end() -> color_eyre::Result<()> {
        let cover_url = serve_once(b"COVER".to_vec()).await?;
        let channel = CannedChannel::empty();
        let queue = TaggingQueue::spawn(
            /*enabled*/ true,
            &[Arc::new(channel)],
            Some(&reqwest::Client::new()),
            /*workers*/ 1,
        );
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("tone.mp3");
        std::fs::write(&path, include_bytes!("fixtures/tone.mp3"))?;
        queue.enqueue(
            song("186016", Some(cover_url)),
            path.clone(),
            BitRate::Lossless,
        );
        wait_until(|| file_has_title(&path), Duration::from_secs(10)).await;
        // 读回标题与封面(罐头专辑 / 歌词为 None,对应字段缺省)。
        let mut f = std::fs::File::open(&path)?;
        let tagged = lofty::probe::Probe::new(&mut f).guess_file_type()?.read()?;
        let tag = tagged
            .primary_tag()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 primary tag"))?;
        assert_eq!(tag.title().as_deref(), Some("晴天"));
        assert!(
            tag.get_picture_type(lofty::picture::PictureType::CoverFront)
                .is_some_and(|p| p.data() == b"COVER"),
            "封面应来自 cover_url GET"
        );
        Ok(())
    }

    /// 去重:同曲同音质连投三次,worker 只处理一次(歌词采集被调次数为证)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enqueue_dedups_inflight() -> color_eyre::Result<()> {
        let channel = CannedChannel {
            album: None,
            lyrics: Some(mineral_model::Lyrics {
                lines: vec![mineral_model::LyricLine::timed(1000, "第一句")],
            }),
            detail_songs: Vec::new(),
            lyrics_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            album_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let calls = Arc::clone(&channel.lyrics_calls);
        let queue = TaggingQueue::spawn(
            /*enabled*/ true,
            &[Arc::new(channel)],
            /*http*/ None,
            /*workers*/ 1,
        );
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("tone.mp3");
        std::fs::write(&path, include_bytes!("fixtures/tone.mp3"))?;
        for _ in 0..3 {
            queue.enqueue(
                song("186016", /*cover*/ None),
                path.clone(),
                BitRate::Lossless,
            );
        }
        // 等第一次处理完(歌词被调过)+ inflight 清空,再补投一次验证可重投。
        wait_until(
            || calls.load(Ordering::Acquire) >= 1,
            Duration::from_secs(10),
        )
        .await;
        wait_until(
            || {
                queue
                    .inner
                    .as_ref()
                    .is_some_and(|i| i.inflight.lock().is_empty())
            },
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "在队 / 在写期间重复投递应去重"
        );
        queue.enqueue(
            song("186016", /*cover*/ None),
            path.clone(),
            BitRate::Lossless,
        );
        wait_until(
            || calls.load(Ordering::Acquire) >= 2,
            Duration::from_secs(10),
        )
        .await;
        Ok(())
    }

    /// 开关关闭 = null-object:投递 no-op,文件保持原样。
    #[tokio::test]
    async fn disabled_queue_is_noop() -> color_eyre::Result<()> {
        let queue = TaggingQueue::spawn(
            /*enabled*/ false,
            &[],
            /*http*/ None,
            /*workers*/ 1,
        );
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("tone.mp3");
        let bytes = include_bytes!("fixtures/tone.mp3");
        std::fs::write(&path, bytes)?;
        queue.enqueue(
            song("186016", /*cover*/ None),
            path.clone(),
            BitRate::Lossless,
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(std::fs::read(&path)?, bytes, "关闭时文件不应被改写");
        Ok(())
    }

    /// 打标失败(垃圾内容)不毒化队列:下一首正常处理。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failure_does_not_poison_queue() -> color_eyre::Result<()> {
        let channel = CannedChannel::empty();
        let queue = TaggingQueue::spawn(
            /*enabled*/ true,
            &[Arc::new(channel)],
            /*http*/ None,
            /*workers*/ 1,
        );
        let dir = tempfile::tempdir()?;
        let garbage = dir.path().join("garbage.bin");
        std::fs::write(&garbage, b"NOT-AUDIO")?;
        let good = dir.path().join("tone.mp3");
        std::fs::write(&good, include_bytes!("fixtures/tone.mp3"))?;
        queue.enqueue(
            song("186016", /*cover*/ None),
            garbage.clone(),
            BitRate::Lossless,
        );
        queue.enqueue(
            song("186017", /*cover*/ None),
            good.clone(),
            BitRate::Lossless,
        );
        wait_until(|| file_has_title(&good), Duration::from_secs(10)).await;
        assert_eq!(std::fs::read(&garbage)?, b"NOT-AUDIO", "失败文件应保持原样");
        Ok(())
    }
}
