//! tag 采集:把一首歌的内嵌 metadata 从 [`Song`] 自身字段、channel(专辑详情 /
//! 歌词)与 HTTP(封面字节)凑齐。
//!
//! 三路采集(专辑详情 / 歌词 / 封面)互不依赖,`tokio::join!` 并发;专辑详情按
//! qualified album id 缓存(worker 共享,一张专辑多首只拉一次,失败也缓存——
//! 单次运行内不对坏专辑反复重试)。
//!
//! 全部 best-effort:单项失败降级为缺字段——对应 tag 不写,其余字段照常落盘。
//! 但**可重试失败**(网络错误等,非能力缺失)会让整体标记 `degraded`:写引擎据此
//! 不写打标水印,下次回填自动重试本文件。

use std::sync::Arc;
use std::time::Duration;

use mineral_channel_core::{Error as ChannelError, MusicChannel};
use mineral_model::{MediaUrl, Song};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use super::write::SongTags;

/// 封面 GET 超时(并发 worker 一单卡死会占住整条 lane)。
const COVER_TIMEOUT: Duration = Duration::from_secs(15);

/// 专辑侧 tag 字段(专辑艺人 / 发行年 / 厂牌;仅在模块内构造,经 [`AlbumCache`] 共享)。
#[derive(Clone, Debug, Default)]
pub(crate) struct AlbumTags {
    /// 专辑艺人(主在前)。
    artists: Vec<String>,

    /// 发行年(`publish_time_ms` 按北京时区折算)。
    year: Option<u32>,

    /// 厂牌(`company`)。
    label: Option<String>,
}

/// 专辑详情缓存:qualified album id → (字段, 是否可重试失败)。
/// worker 共享;一次运行内同一张专辑只拉一次(含失败)。
pub(crate) type AlbumCache = Arc<Mutex<FxHashMap<String, (Option<AlbumTags>, bool)>>>;

/// 采集一首歌的 [`SongTags`]:自有字段直接映射;专辑 / 歌词 / 封面三路并发补齐。
///
/// # Params:
///   - `channel`: 该歌来源的 channel(拉专辑详情 / 歌词)
///   - `http`: 下载用 HTTP client(GET 封面;`None` = 跳过封面)
///   - `song`: 目标歌曲
///   - `album_cache`: 专辑详情缓存(worker 共享)
///
/// # Return:
///   `(tags, degraded)`:`degraded = true` 表示有「可重试失败」,调用方不应写水印。
pub(crate) async fn collect(
    channel: &dyn MusicChannel,
    http: Option<&reqwest::Client>,
    song: &Song,
    album_cache: &AlbumCache,
) -> (SongTags, bool) {
    let (album, lyrics, cover) = tokio::join!(
        fetch_album(channel, song, album_cache),
        fetch_lyrics(channel, song),
        fetch_cover(http, song),
    );
    let degraded = album.1 || lyrics.1 || cover.1;
    let mut tags = SongTags {
        title: (!song.name.is_empty()).then(|| song.name.clone()),
        artists: song
            .artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>(),
        album: song.album.as_ref().map(|a| a.name.clone()),
        lyrics_lrc: lyrics.0,
        cover: cover.0,
        ..SongTags::default()
    };
    if let Some(album_tags) = album.0 {
        tags.album_artists = album_tags.artists;
        tags.year = album_tags.year;
        tags.label = album_tags.label;
    }
    (tags, degraded)
}

/// 拉专辑字段(带缓存 + 限流退避):无专辑引用 → `(None, 非失败)`;`NotSupported` = 能力
/// 缺失,不算可重试失败。
///
/// # Params:
///   - `channel`: 该歌来源的 channel
///   - `song`: 目标歌曲
///   - `cache`: 专辑详情缓存
///
/// # Return:
///   `(字段, 是否可重试失败)`。
async fn fetch_album(
    channel: &dyn MusicChannel,
    song: &Song,
    cache: &AlbumCache,
) -> (Option<AlbumTags>, bool) {
    let Some(album_ref) = song.album.as_ref() else {
        return (None, false);
    };
    let key = album_ref.id.qualified();
    if let Some(hit) = cache.lock().get(&key) {
        return hit.clone();
    }
    let result = match with_backoff(|| channel.album_detail(&album_ref.id)).await {
        Ok(album) => (
            Some(AlbumTags {
                artists: album
                    .artists
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>(),
                year: publish_year(album.publish_time_ms).and_then(|y| u32::try_from(y).ok()),
                label: album.company.clone(),
            }),
            false,
        ),
        Err(e) => {
            let retryable = !matches!(e, ChannelError::NotSupported);
            mineral_log::warn!(target: "tagging", song_id = song.id.as_str(), error = mineral_log::chain(&e), "拉专辑详情失败,专辑字段缺省");
            (None, retryable)
        }
    };
    cache.lock().insert(key, result.clone());
    result
}

/// 拉歌词:结构化歌词 → lrc 文本(带限流退避)。源无歌词能力(NotSupported)是常态,
/// 不记日志。
///
/// # Return:
///   `(lrc 文本, 是否可重试失败)`。
async fn fetch_lyrics(channel: &dyn MusicChannel, song: &Song) -> (Option<String>, bool) {
    match with_backoff(|| channel.lyrics(&song.id)).await {
        Ok(lyrics) if !lyrics.lines.is_empty() => {
            (Some(mineral_model::to_lrc_string(&lyrics.lines)), false)
        }
        Ok(_) => (None, false),
        Err(ChannelError::NotSupported) => (None, false),
        Err(e) => {
            mineral_log::warn!(target: "tagging", song_id = song.id.as_str(), error = mineral_log::chain(&e), "拉歌词失败,歌词字段缺省");
            (None, true)
        }
    }
}

/// 限流退避序列(2s / 5s / 15s;打标是后台任务,等得起)。
const BACKOFFS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// 限流退避包装:channel 调用命中 `RateLimited` 时按 [`BACKOFFS`] 退避重试,其余结果
/// (成功 / 其他错误)原样返回。限流是服务端临时状态——立刻当失败会让本文件永远
/// 拿不到水印、每次回填都白跑一遍。
///
/// # Params:
///   - `call`: channel 调用工厂(重试时重新调用)
///
/// # Return:
///   最终一次调用的结果(退避耗尽仍限流 → `Err(RateLimited)`)。
async fn with_backoff<T, Fut, F>(call: F) -> std::result::Result<T, mineral_channel_core::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, mineral_channel_core::Error>>,
{
    with_backoff_in(call, BACKOFFS).await
}

/// [`with_backoff`] 的可注入退避序列版(测试用零时长)。
async fn with_backoff_in<T, Fut, F>(
    mut call: F,
    backoffs: impl IntoIterator<Item = Duration>,
) -> std::result::Result<T, mineral_channel_core::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, mineral_channel_core::Error>>,
{
    let mut backoffs = backoffs.into_iter();
    loop {
        match call().await {
            Err(e) if matches!(e, ChannelError::RateLimited) => {
                let Some(delay) = backoffs.next() else {
                    return Err(e);
                };
                mineral_log::warn!(target: "tagging", delay_ms = delay.as_millis(), "限流,退避重试");
                tokio::time::sleep(delay).await;
            }
            result => return result,
        }
    }
}

/// 拉封面:GET `cover_url` 字节(mime 留待写引擎按字节 sniff)。
///
/// # Return:
///   `(图片字节, 是否可重试失败)`。
async fn fetch_cover(http: Option<&reqwest::Client>, song: &Song) -> (Option<Vec<u8>>, bool) {
    let (Some(http), Some(MediaUrl::Remote(url))) = (http, &song.cover_url) else {
        return (None, false);
    };
    let result = async {
        let resp = http
            .get(url.clone())
            .timeout(COVER_TIMEOUT)
            .send()
            .await?
            .error_for_status()?;
        resp.bytes().await
    }
    .await;
    match result {
        Ok(bytes) => (Some(bytes.to_vec()), false),
        Err(e) => {
            mineral_log::warn!(target: "tagging", song_id = song.id.as_str(), error = mineral_log::chain(&e), "拉封面失败,封面字段缺省");
            (None, true)
        }
    }
}

/// epoch ms → 发行年(北京时区;与 TUI 侧 `publish_year` 口径一致,见
/// `crates/mineral-tui/src/components/layout/search/detail/meta.rs`——两处语义
/// 必须同步,UTC 直读会在北京跨年点错一年)。
///
/// # Params:
///   - `ms`: 发行时间(epoch 毫秒;`<= 0` 视为未知)
///
/// # Return:
///   公历年;未知 / 溢出为 `None`。
fn publish_year(ms: i64) -> Option<i32> {
    if ms <= 0 {
        return None;
    }
    let beijing = time::UtcOffset::from_hms(8, 0, 0).ok()?;
    let dt = time::OffsetDateTime::from_unix_timestamp(ms / 1000).ok()?;
    Some(dt.to_offset(beijing).year())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use mineral_model::{
        Album, AlbumId, AlbumRef, ArtistId, ArtistRef, LyricLine, Lyrics, SongId, SourceKind,
    };
    use mineral_test::mock::{CannedChannel, serve_once};

    use super::*;

    /// 带全量元数据的测试歌曲(双艺人 + 专辑引用 + 封面 URL)。
    fn song(cover: Option<url::Url>) -> Song {
        Song::builder()
            .id(SongId::new(SourceKind::NETEASE, "186016"))
            .name("晴天".to_owned())
            .artists(vec![
                ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, "6452"),
                    name: "周杰伦".to_owned(),
                },
                ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, "0"),
                    name: "第二艺人".to_owned(),
                },
            ])
            .album(Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "31655"),
                name: "叶惠美".to_owned(),
            }))
            .cover_url(cover.map(MediaUrl::Remote))
            .build()
    }

    /// 罐头专辑:双专辑艺人 + 发行时间 + 厂牌。
    fn album() -> Album {
        Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, "31655"))
            .name("叶惠美".to_owned())
            .artists(vec![
                ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, "6452"),
                    name: "周杰伦".to_owned(),
                },
                ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, "0"),
                    name: "专辑合艺人".to_owned(),
                },
            ])
            .company(Some("杰威尔".to_owned()))
            .publish_time_ms(1_059_379_200_000) // 2003-07-31 北京
            .build()
    }

    /// 罐头歌词:两行带时间戳。
    fn lyrics() -> Lyrics {
        Lyrics {
            lines: vec![
                LyricLine::timed(1000, "故事的小黄花"),
                LyricLine::timed(5000, "从出生那年就飘着"),
            ],
        }
    }

    /// 空专辑缓存。
    fn cache() -> AlbumCache {
        Arc::new(Mutex::new(FxHashMap::default()))
    }

    /// 全链路采集:自有字段 + 专辑 + 歌词 + 封面(本地一次性 server)全部到位,无 degraded。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn collect_full() -> color_eyre::Result<()> {
        let cover_url = serve_once(b"PNG-BYTES".to_vec()).await?;
        let channel = CannedChannel {
            album: Some(album()),
            lyrics: Some(lyrics()),
            ..CannedChannel::empty()
        };
        let http = reqwest::Client::new();
        let (tags, degraded) =
            collect(&channel, Some(&http), &song(Some(cover_url)), &cache()).await;
        assert!(!degraded, "全部成功不应 degraded");
        assert_eq!(tags.title.as_deref(), Some("晴天"));
        assert_eq!(tags.artists, vec!["周杰伦", "第二艺人"]);
        assert_eq!(tags.album.as_deref(), Some("叶惠美"));
        assert_eq!(tags.album_artists, vec!["周杰伦", "专辑合艺人"]);
        assert_eq!(tags.year, Some(2003));
        assert_eq!(tags.label.as_deref(), Some("杰威尔"));
        let lrc = tags.lyrics_lrc.unwrap_or_default();
        assert!(
            lrc.contains("故事的小黄花") && lrc.contains("[00:01.00]"),
            "歌词应序列化为 lrc: {lrc}"
        );
        assert_eq!(tags.cover.as_deref(), Some(&b"PNG-BYTES"[..]));
        Ok(())
    }

    /// 专辑缓存:同一张专辑的两首歌,`album_detail` 只拉一次。
    #[tokio::test]
    async fn album_detail_is_cached_per_album() -> color_eyre::Result<()> {
        let channel = CannedChannel {
            album: Some(album()),
            ..CannedChannel::empty()
        };
        let calls = Arc::clone(&channel.album_calls);
        let cache = cache();
        let s1 = song(/*cover*/ None);
        let mut s2 = song(/*cover*/ None);
        s2.id = SongId::new(SourceKind::NETEASE, "186017");
        collect(&channel, /*http*/ None, &s1, &cache).await;
        collect(&channel, /*http*/ None, &s2, &cache).await;
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "同专辑第二首应命中缓存,不再拉取"
        );
        Ok(())
    }

    /// 单项失败降级:专辑详情 / 歌词 / 封面全挂,自有字段不受影响;
    /// 封面 GET 失败是可重试失败 → degraded(不写水印)。
    #[tokio::test]
    async fn failures_degrade_to_missing_fields() -> color_eyre::Result<()> {
        // 封面指向无人监听的端口 → GET 必失败。
        let (tags, degraded) = collect(
            &CannedChannel::empty(),
            Some(&reqwest::Client::new()),
            &song(Some("http://127.0.0.1:9/dead.jpg".parse()?)),
            &cache(),
        )
        .await;
        assert!(degraded, "封面 GET 失败应标 degraded(不写水印)");
        assert_eq!(tags.title.as_deref(), Some("晴天"));
        assert_eq!(tags.artists, vec!["周杰伦", "第二艺人"]);
        assert_eq!(tags.album.as_deref(), Some("叶惠美"));
        assert!(tags.album_artists.is_empty(), "专辑详情失败 → 专辑艺人缺省");
        assert_eq!(tags.year, None, "专辑详情失败 → 年份缺省");
        assert_eq!(tags.label, None, "专辑详情失败 → 厂牌缺省");
        assert_eq!(tags.lyrics_lrc, None, "歌词不支持 → 歌词缺省");
        assert_eq!(tags.cover, None, "封面 GET 失败 → 封面缺省");
        Ok(())
    }

    /// 能力缺失(NotSupported)不算可重试失败 → 不 degraded(照写水印,不反复重试)。
    #[tokio::test]
    async fn not_supported_is_not_degraded() -> color_eyre::Result<()> {
        let (tags, degraded) = collect(
            &CannedChannel::empty(),
            /*http*/ None,
            &song(/*cover*/ None),
            &cache(),
        )
        .await;
        assert!(!degraded, "NotSupported 是能力缺失,不应 degraded");
        assert_eq!(tags.title.as_deref(), Some("晴天"));
        Ok(())
    }

    /// 限流退避:`RateLimited` 重试到成功;退避耗尽返回限流错误;其他错误不重试。
    #[tokio::test]
    async fn backoff_retries_rate_limited() -> color_eyre::Result<()> {
        let zero = [Duration::ZERO; 3];
        // 两次限流后成功:应重试并最终拿到值。
        let calls = AtomicUsize::new(0);
        let r: std::result::Result<u32, ChannelError> = with_backoff_in(
            || {
                calls.fetch_add(1, Ordering::AcqRel);
                std::future::ready(if calls.load(Ordering::Acquire) < 3 {
                    Err(ChannelError::RateLimited)
                } else {
                    Ok(7)
                })
            },
            zero,
        )
        .await;
        assert_eq!(r.ok(), Some(7));
        assert_eq!(calls.load(Ordering::Acquire), 3);
        // 持续限流:退避耗尽后返回 RateLimited(共 1 + 3 次调用)。
        let calls2 = AtomicUsize::new(0);
        let r2: std::result::Result<u32, ChannelError> = with_backoff_in(
            || {
                calls2.fetch_add(1, Ordering::AcqRel);
                std::future::ready(Err(ChannelError::RateLimited))
            },
            zero,
        )
        .await;
        assert!(matches!(r2, Err(ChannelError::RateLimited)));
        assert_eq!(calls2.load(Ordering::Acquire), 4, "退避序列用完后放弃");
        // 其他错误(非限流)不重试。
        let calls3 = AtomicUsize::new(0);
        let r3: std::result::Result<u32, ChannelError> = with_backoff_in(
            || {
                calls3.fetch_add(1, Ordering::AcqRel);
                std::future::ready(Err(ChannelError::NotSupported))
            },
            zero,
        )
        .await;
        assert!(matches!(r3, Err(ChannelError::NotSupported)));
        assert_eq!(calls3.load(Ordering::Acquire), 1, "非限流错误不应重试");
        Ok(())
    }

    #[test]
    fn publish_year_uses_beijing_offset() {
        assert_eq!(publish_year(1_059_379_200_000), Some(2003));
        // 2020-01-01 00:00 北京(= 2019-12-31 16:00 UTC)→ 2020;UTC 读会错成 2019。
        assert_eq!(publish_year(1_577_808_000_000), Some(2020));
        assert_eq!(publish_year(0), None);
        assert_eq!(publish_year(-1), None);
    }
}
