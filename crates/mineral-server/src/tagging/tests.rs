//! 打标队列的端到端、去重、关闭开关与失败恢复验证。

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use color_eyre::eyre::eyre;
use lofty::file::TaggedFileExt as _;
use lofty::tag::Accessor as _;
use mineral_channel_core::{ChannelCaps, MusicChannel, Result as ChannelResult};
use mineral_model::{AlbumRef, ArtistRef, BitRate, CachePolicy, Lyrics, Song, SongId, SourceKind};
use mineral_test::mock::{CannedChannel, serve_once};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use super::TaggingQueue;

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
    tokio::fs::write(&path, include_bytes!("fixtures/tone.mp3")).await?;
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

/// 通过歌词请求控制处理顺序，验证去重、完成后重投与关闭投递端后排空队列。
#[tokio::test]
async fn enqueue_dedups_inflight() -> color_eyre::Result<()> {
    struct ControlledLyricsChannel {
        requests: mpsc::UnboundedSender<(SongId, oneshot::Sender<()>)>,
    }

    #[async_trait]
    impl MusicChannel for ControlledLyricsChannel {
        fn source(&self) -> SourceKind {
            SourceKind::NETEASE
        }

        fn caps(&self) -> ChannelCaps {
            CannedChannel::empty().caps()
        }

        async fn songs_detail(
            &self,
            ids: &[SongId],
            cache: CachePolicy,
        ) -> ChannelResult<Vec<Song>> {
            CannedChannel::empty().songs_detail(ids, cache).await
        }

        async fn lyrics(&self, id: &SongId) -> ChannelResult<Lyrics> {
            let (finish, wait) = oneshot::channel();
            assert!(self.requests.send((id.clone(), finish)).is_ok());
            assert!(wait.await.is_ok());
            CannedChannel::empty().lyrics(id).await
        }
    }

    let (requests, mut received) = mpsc::unbounded_channel();
    let queue = TaggingQueue::spawn(
        /*enabled*/ true,
        &[Arc::new(ControlledLyricsChannel { requests })],
        /*http*/ None,
        /*workers*/ 1,
    );
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tone.mp3");
    let next_path = dir.path().join("next.mp3");
    tokio::fs::write(&path, include_bytes!("fixtures/tone.mp3")).await?;
    tokio::fs::write(&next_path, include_bytes!("fixtures/tone.mp3")).await?;
    for _ in 0..3 {
        queue.enqueue(
            song("186016", /*cover*/ None),
            path.clone(),
            BitRate::Lossless,
        );
    }
    queue.enqueue(
        song("186017", /*cover*/ None),
        next_path.clone(),
        BitRate::Lossless,
    );
    // 单线程运行时：调度任务接收上述请求后，停在未放行的歌词请求上。
    let (id, finish) = timeout(Duration::from_secs(10), received.recv())
        .await?
        .ok_or_else(|| eyre!("第一首应开始处理"))?;
    assert_eq!(id.as_str(), "186016");
    assert!(received.try_recv().is_err(), "并发数为 1 时下一首应等待");
    assert!(finish.send(()).is_ok());

    let (id, finish) = timeout(Duration::from_secs(10), received.recv())
        .await?
        .ok_or_else(|| eyre!("第二首应开始处理"))?;
    assert_eq!(id.as_str(), "186017", "重复请求不应再次采集歌词");
    assert!(file_has_title(&path), "下一首开始时前一首应已写回");

    // 下一首开始意味着前一首已完成；恢复无 tag 文件后重投，验证再次写回。
    tokio::fs::write(&path, include_bytes!("fixtures/tone.mp3")).await?;
    queue.enqueue(
        song("186016", /*cover*/ None),
        path.clone(),
        BitRate::Lossless,
    );
    drop(queue);
    assert!(finish.send(()).is_ok());

    let (id, finish) = timeout(Duration::from_secs(10), received.recv())
        .await?
        .ok_or_else(|| eyre!("关闭投递端后仍应处理重投任务"))?;
    assert_eq!(id.as_str(), "186016");
    assert!(file_has_title(&next_path));
    assert!(finish.send(()).is_ok());
    assert!(
        timeout(Duration::from_secs(10), received.recv())
            .await?
            .is_none(),
        "队列排空后应退出"
    );
    assert!(file_has_title(&path));
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
    tokio::fs::write(&path, bytes).await?;
    queue.enqueue(
        song("186016", /*cover*/ None),
        path.clone(),
        BitRate::Lossless,
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(tokio::fs::read(&path).await?, bytes, "关闭时文件不应被改写");
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
    tokio::fs::write(&garbage, b"NOT-AUDIO").await?;
    let good = dir.path().join("tone.mp3");
    tokio::fs::write(&good, include_bytes!("fixtures/tone.mp3")).await?;
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
    assert_eq!(
        tokio::fs::read(&garbage).await?,
        b"NOT-AUDIO",
        "失败文件应保持原样"
    );
    Ok(())
}
