//! 落盘歌曲的内嵌 metadata 打标:side 子系统,下载 / 缓存落盘后异步写 tag。
//!
//! 投递方只发消息不等待;N 个并发 worker(配置 `download.tagging_workers`)共享一个
//! mpsc receiver 抢单,按 `{song_id.qualified()}:{quality}:{path}` 去重在队 / 在写
//! 任务。单曲采集(专辑详情 / 歌词 / 封面)三路并发,专辑详情按专辑缓存(同一张专辑
//! 多首只拉一次)。写盘成功的文件带版本化水印(`EncodedBy`),回填据此增量跳过;有
//! 可重试失败的单曲不写水印,下次自动重试。失败只记日志——不影响下载与播放。

mod assemble;
mod write;

use std::path::PathBuf;
use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, Song, SongId, SourceKind};
use parking_lot::Mutex;
use rustc_hash::FxHashSet;

use crate::media_cache::cache_key;

/// 打标任务的歌曲身份。
enum JobIdentity {
    /// 完整元数据(新落盘路径:下载 / 收割时手上就有)。`Box` 避免 enum 体积膨胀。
    Full(Box<Song>),

    /// 仅 id(存量回填):worker 侧先经 persist / channel 解析出 [`Song`] 再打标。
    Ref(SongId),
}

/// 一条打标任务(落盘文件 + 其歌曲身份)。
struct TagJob {
    /// 歌曲身份。
    identity: JobIdentity,

    /// 落盘文件(导出或缓存库路径)。
    path: PathBuf,

    /// 入库音质(去重键维度之一)。
    quality: BitRate,
}

impl TagJob {
    /// 歌曲 id(去重键 / 日志用)。
    fn song_id(&self) -> &SongId {
        match &self.identity {
            JobIdentity::Full(s) => &s.id,
            JobIdentity::Ref(id) => id,
        }
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

    /// 进度计数(worker 与查询端共享;daemon 生命周期内单调累计)。
    progress: Arc<Progress>,
}

/// 打标进度计数(原子;受理 / 处理完 / 其中失败)。
#[derive(Default)]
struct Progress {
    /// 已受理(未被去重丢弃)的任务数。
    submitted: std::sync::atomic::AtomicU64,

    /// 已处理完(成功 + 容器不支持跳过 + 失败)的任务数。
    processed: std::sync::atomic::AtomicU64,

    /// 其中失败数(写盘错误)。
    failed: std::sync::atomic::AtomicU64,
}

/// 打标进度快照(累计值;`processed - failed` = 成功或跳过数)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TagProgress {
    /// 已受理任务数。
    pub(crate) submitted: u64,

    /// 已处理完任务数。
    pub(crate) processed: u64,

    /// 其中失败数。
    pub(crate) failed: u64,
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
    ///   - `persist`: 持久化句柄(回填任务按 id 反查 `song_meta`;worker 持 clone)
    ///   - `workers`: 并发 worker 数(配置 `download.tagging_workers`;`<1` 按 1)
    ///
    /// # Return:
    ///   投递句柄(廉价 clone)。
    pub(crate) fn spawn(
        enabled: bool,
        channels: &[Arc<dyn MusicChannel>],
        http: Option<&reqwest::Client>,
        persist: &mineral_persist::ServerStore,
        workers: usize,
    ) -> Self {
        if !enabled {
            return Self { inner: None };
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // mpsc 单消费者:多 worker 共享一个 receiver(锁只护 recv,不跨任务处理)。
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let inflight = Arc::new(Mutex::new(FxHashSet::default()));
        let progress = Arc::new(Progress::default());
        let album_cache = assemble::AlbumCache::default();
        for _ in 0..workers.max(1) {
            tokio::spawn(run(
                Arc::clone(&rx),
                channels.to_vec(),
                http.cloned(),
                persist.clone(),
                Arc::clone(&inflight),
                Arc::clone(&progress),
                album_cache.clone(),
            ));
        }
        Self {
            inner: Some(Arc::new(QueueInner {
                tx,
                inflight,
                progress,
            })),
        }
    }

    /// 打标进度快照(累计计数;开关关闭恒零)。
    ///
    /// # Return:
    ///   受理 / 处理完 / 失败三元组。
    pub(crate) fn progress(&self) -> TagProgress {
        let Some(inner) = &self.inner else {
            return TagProgress::default();
        };
        use std::sync::atomic::Ordering::Acquire;
        TagProgress {
            submitted: inner.progress.submitted.load(Acquire),
            processed: inner.progress.processed.load(Acquire),
            failed: inner.progress.failed.load(Acquire),
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
                identity: JobIdentity::Full(Box::new(song)),
                path,
                quality,
            },
            &key,
        )
    }

    /// 投递一条 identity-only 回填任务(仅 id;元数据由 worker 经 persist / channel 解析)。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///   - `path`: 落盘文件路径
    ///   - `quality`: 入库音质
    ///
    /// # Return:
    ///   `true` = 已受理(未去重);`false` = 去重丢弃 / 开关关闭。
    pub(crate) fn enqueue_ref(&self, id: SongId, path: PathBuf, quality: BitRate) -> bool {
        let key = job_key(&id, quality, &path);
        self.send(
            TagJob {
                identity: JobIdentity::Ref(id),
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
        inner
            .progress
            .submitted
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        true
    }
}

/// worker 主循环:从共享 receiver 取任务,逐首采集 + 写盘;单首的任何失败只记日志。
///
/// # Params:
///   - `rx`: 共享任务接收端(多 worker 抢单,锁只护 `recv` 调用本身)
///   - `channels`: 已注入的 channel(按歌曲来源路由)
///   - `http`: 下载用 HTTP client(GET 封面)
///   - `persist`: 持久化句柄(回填任务按 id 反查 `song_meta`)
///   - `inflight`: 在队 / 在写集合(消费完剔除)
///   - `progress`: 进度计数(消费完 +1,失败再 +1)
///   - `album_cache`: 专辑详情缓存(worker 池共享)
async fn run(
    rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<TagJob>>>,
    channels: Vec<Arc<dyn MusicChannel>>,
    http: Option<reqwest::Client>,
    persist: mineral_persist::ServerStore,
    inflight: Arc<Mutex<FxHashSet<String>>>,
    progress: Arc<Progress>,
    album_cache: assemble::AlbumCache,
) {
    use std::sync::atomic::Ordering::AcqRel;
    loop {
        // 锁的临时 guard 在本语句结束即释放,不跨任务处理。
        let job = rx.lock().await.recv().await;
        let Some(job) = job else { break };
        let outcome = process(&job, &channels, http.as_ref(), &persist, &album_cache).await;
        progress.processed.fetch_add(1, AcqRel);
        if matches!(outcome, JobOutcome::Failed) {
            progress.failed.fetch_add(1, AcqRel);
        }
        inflight
            .lock()
            .remove(&job_key(job.song_id(), job.quality, &job.path));
    }
}

/// 单首打标的结局(进度计数用)。
enum JobOutcome {
    /// 已写入(或无需写也视作完成)。
    Done,

    /// 失败(写盘错误等)。
    Failed,
}

/// 打标一首:解析身份(回填任务)→ 三路并发采集 → `spawn_blocking` 写盘。
///
/// # Params:
///   - `job`: 打标任务
///   - `channels`: 已注入的 channel
///   - `http`: 下载用 HTTP client
///   - `persist`: 持久化句柄
///   - `album_cache`: 专辑详情缓存
///
/// # Return:
///   结局(仅失败 / 非失败两分;日志已含细分原因)。
async fn process(
    job: &TagJob,
    channels: &[Arc<dyn MusicChannel>],
    http: Option<&reqwest::Client>,
    persist: &mineral_persist::ServerStore,
    album_cache: &assemble::AlbumCache,
) -> JobOutcome {
    let Some(channel) = channels
        .iter()
        .find(|ch| ch.source() == job.song_id().namespace())
    else {
        mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), "无对应 channel,跳过打标");
        return JobOutcome::Failed;
    };
    let song = match &job.identity {
        JobIdentity::Full(s) => Some(s.as_ref().clone()),
        JobIdentity::Ref(id) => resolve_song(channel.as_ref(), persist, id).await,
    };
    let Some(song) = song else {
        mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), "元数据解析失败,跳过打标");
        return JobOutcome::Failed;
    };
    let (tags, degraded) = assemble::collect(channel.as_ref(), http, &song, album_cache).await;
    let path = job.path.clone();
    // 有可重试失败不写水印:本文件下次回填还会重试,不会带半成品沉淀。
    let result = tokio::task::spawn_blocking(move || {
        write::write_tags(&path, &tags, /*watermark*/ !degraded)
    })
    .await;
    match result {
        Ok(Ok(write::WriteOutcome::Tagged)) => {
            mineral_log::info!(target: "tagging", song_id = job.song_id().as_str(), path = %job.path.display(), "已写入内嵌 tag");
            JobOutcome::Done
        }
        Ok(Ok(write::WriteOutcome::SkippedUnsupported)) => {
            mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), path = %job.path.display(), "内容无法探测或容器不支持,跳过打标");
            JobOutcome::Done
        }
        Ok(Err(e)) => {
            mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), error = mineral_log::chain(&e), "打标失败");
            JobOutcome::Failed
        }
        Err(e) => {
            mineral_log::warn!(target: "tagging", song_id = job.song_id().as_str(), error = mineral_log::chain(&e), "打标任务被取消");
            JobOutcome::Failed
        }
    }
}

/// 由 id 解析完整 [`Song`]:persist `song_meta` 优先(本地、免费),未命中经 channel
/// `songs_detail` 补拉(一次网络请求)。
///
/// # Params:
///   - `channel`: 该曲来源的 channel
///   - `persist`: 持久化句柄
///   - `id`: 歌曲 id
///
/// # Return:
///   解析出的 Song;两路都拿不到返回 `None`(调用方跳过)。
async fn resolve_song(
    channel: &dyn MusicChannel,
    persist: &mineral_persist::ServerStore,
    id: &SongId,
) -> Option<Song> {
    match persist.scope(id.namespace()).get_meta(id).await {
        Ok(Some(song)) => return Some(song),
        Ok(None) => {}
        Err(e) => {
            mineral_log::warn!(target: "tagging", song_id = id.as_str(), error = mineral_log::chain(&e), "读 song_meta 失败,改走 channel 补拉");
        }
    }
    match channel.songs_detail(std::slice::from_ref(id)).await {
        Ok(songs) => songs.into_iter().next(),
        Err(e) => {
            mineral_log::warn!(target: "tagging", song_id = id.as_str(), error = mineral_log::chain(&e), "songs_detail 补拉失败");
            None
        }
    }
}

/// 回填结果计数(经 IPC 回给 CLI)。计数语义是**受理数**(未被去重丢弃、开关开启),
/// 供 CLI 换算进度终点。
pub(crate) struct BackfillCounts {
    /// 缓存侧受理数。
    pub(crate) cached: u32,

    /// 导出侧受理数。
    pub(crate) exported: u32,
}

/// 存量回填:枚举缓存索引(全部在盘条目)+ 导出侧两路 inventory,逐文件投递打标。
/// 去重、元数据解析、失败日志与新落盘路径同一条队列语义;只投递,不等待。
///
/// # Params:
///   - `player`: 播放核心(取 media_cache / stats / persist / music_dir / tagging 队列)
///
/// # Return:
///   两侧受理计数(去重丢弃 / 开关关闭不计入)。
pub(crate) async fn backfill(
    player: &crate::player::PlayerCore,
) -> color_eyre::Result<BackfillCounts> {
    let cached = player.media_cache().entries();
    let mut exported = player.inner.stats.store().successful_downloads().await?;
    if let Some(music_dir) = player.music_dir() {
        let sources = player
            .channels()
            .iter()
            .map(|ch| ch.source())
            .collect::<Vec<_>>();
        exported.extend(export_candidates(player.persist(), music_dir, &sources).await?);
    }
    let mut counts = BackfillCounts {
        cached: 0,
        exported: 0,
    };
    // 回填在投递前同步逐文件探测水印;已带当前版本水印的文件跳过。
    for (id, quality, path) in cached {
        if write::has_watermark(&path) {
            continue;
        }
        counts.cached += u32::from(player.tagging().enqueue_ref(id, path, quality));
    }
    for (id, quality, path) in exported {
        if write::has_watermark(&path) {
            continue;
        }
        counts.exported += u32::from(player.tagging().enqueue_ref(id, path, quality));
    }
    Ok(counts)
}

/// 导出侧候选(song_meta 驱动):枚举各 source 的 `song_meta`,逐歌逐音质
/// [`crate::resolve::probe_export`] 命中即候选。补上 stats downloads 之外的旧下载
/// (下载本身不写 `song_meta`,但歌单缓存 / 收藏回填会写,覆盖率高一个量级)。
///
/// # Params:
///   - `persist`: 持久化句柄
///   - `music_dir`: 导出根目录
///   - `sources`: 已注册 channel 的 source(决定枚举哪些 namespace)
///
/// # Return:
///   `(SongId, quality, path)` 候选(同一文件可能来自多个 source 记录,下游按 key 去重)。
async fn export_candidates(
    persist: &mineral_persist::ServerStore,
    music_dir: &std::path::Path,
    sources: &[SourceKind],
) -> color_eyre::Result<Vec<(SongId, BitRate, PathBuf)>> {
    let mut out = Vec::new();
    for source in sources {
        let songs = persist.scope(*source).list_meta().await?;
        for song in &songs {
            for quality in BitRate::ALL {
                if let Some(path) = crate::resolve::probe_export(music_dir, song, quality) {
                    out.push((song.id.clone(), quality, path));
                }
            }
        }
    }
    Ok(out)
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
            &mineral_persist::ServerStore::disabled(),
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

    /// 回填任务(仅 id):persist 有 meta 时直接解析(不经 channel),正常打标。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enqueue_ref_prefers_persist_meta() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = mineral_persist::ServerStore::open(&d.path().join("m.db")).await?;
        let s = song("186016", /*cover*/ None);
        persist.scope(s.id.namespace()).upsert_meta(&s).await?;
        // channel 无 detail_songs:若错走 channel 会解析失败;persist 命中则不碰 channel。
        let channel = CannedChannel::empty();
        let queue = TaggingQueue::spawn(
            /*enabled*/ true,
            &[Arc::new(channel)],
            /*http*/ None,
            &persist,
            /*workers*/ 1,
        );
        let path = d.path().join("tone.mp3");
        std::fs::write(&path, include_bytes!("fixtures/tone.mp3"))?;
        queue.enqueue_ref(s.id.clone(), path.clone(), BitRate::Lossless);
        wait_until(|| file_has_title(&path), Duration::from_secs(10)).await;
        Ok(())
    }

    /// 回填任务(仅 id):persist 无 meta(disabled)时经 channel `songs_detail` 解析再打标。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enqueue_ref_falls_back_to_songs_detail() -> color_eyre::Result<()> {
        let s = song("186016", /*cover*/ None);
        let channel = CannedChannel {
            detail_songs: vec![s.clone()],
            ..CannedChannel::empty()
        };
        let queue = TaggingQueue::spawn(
            /*enabled*/ true,
            &[Arc::new(channel)],
            /*http*/ None,
            &mineral_persist::ServerStore::disabled(),
            /*workers*/ 1,
        );
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("tone.mp3");
        std::fs::write(&path, include_bytes!("fixtures/tone.mp3"))?;
        queue.enqueue_ref(s.id.clone(), path.clone(), BitRate::Lossless);
        wait_until(|| file_has_title(&path), Duration::from_secs(10)).await;
        Ok(())
    }

    /// 导出侧候选:song_meta 里的歌 + 磁盘上按库路径落盘的文件才命中;只有 meta 未落盘的不算。
    #[tokio::test]
    async fn export_candidates_hits_meta_and_disk() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = mineral_persist::ServerStore::open(&d.path().join("m.db")).await?;
        let s = song("186016", /*cover*/ None);
        persist.scope(s.id.namespace()).upsert_meta(&s).await?;
        // 另一首只有 meta、没有落盘文件(名字不命中任何文件)→ 不应成为候选。
        let ghost = Song::builder()
            .id(SongId::new(SourceKind::NETEASE, "186017"))
            .name("不存在之歌".to_owned())
            .build();
        persist
            .scope(ghost.id.namespace())
            .upsert_meta(&ghost)
            .await?;
        let music_dir = d.path().join("music");
        let album_dir = music_dir.join("netease/lossless/叶惠美");
        std::fs::create_dir_all(&album_dir)?;
        let file = album_dir.join("晴天.flac");
        std::fs::write(&file, b"FLAC")?;

        let found = export_candidates(&persist, &music_dir, &[SourceKind::NETEASE]).await?;
        assert_eq!(found.len(), 1, "只应命中一首: {found:?}");
        let Some((id, quality, path)) = found.first() else {
            return Err(color_eyre::eyre::eyre!("应有候选"));
        };
        assert_eq!(id, &s.id);
        assert_eq!(*quality, BitRate::Lossless);
        assert_eq!(path, &file);
        Ok(())
    }

    /// 进度计数:受理 +1、处理完 +1;去重投递不计入;开关关闭恒零。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn progress_counts_accepted_and_processed() -> color_eyre::Result<()> {
        let channel = CannedChannel::empty();
        let queue = TaggingQueue::spawn(
            /*enabled*/ true,
            &[Arc::new(channel)],
            /*http*/ None,
            &mineral_persist::ServerStore::disabled(),
            /*workers*/ 1,
        );
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("tone.mp3");
        std::fs::write(&path, include_bytes!("fixtures/tone.mp3"))?;
        assert!(queue.enqueue(
            song("186016", /*cover*/ None),
            path.clone(),
            BitRate::Lossless
        ));
        assert!(
            !queue.enqueue(
                song("186016", /*cover*/ None),
                path.clone(),
                BitRate::Lossless
            ),
            "同 key 复投应去重"
        );
        assert_eq!(queue.progress().submitted, 1, "去重投递不计入受理");
        wait_until(|| queue.progress().processed == 1, Duration::from_secs(10)).await;
        assert_eq!(queue.progress().failed, 0);
        assert_eq!(
            TaggingQueue::spawn(
                /*enabled*/ false,
                &[],
                None,
                &mineral_persist::ServerStore::disabled(),
                /*workers*/ 1,
            )
            .progress(),
            TagProgress::default(),
            "关闭态进度恒零"
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
            &mineral_persist::ServerStore::disabled(),
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
            &mineral_persist::ServerStore::disabled(),
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
            &mineral_persist::ServerStore::disabled(),
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
