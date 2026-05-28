//! 不依赖播放的下载:给定 [`Song`] → `song_urls` 拿直链 → 整段 HTTP GET → **永久导出**。
//!
//! 这是可复用单元——键位下载单曲 / 歌单批量、将来 gapless 预下载都调 [`download_song`]:
//! 导出落 `<music_dir>/<source>/<quality>/<album>/<title>.<ext>`(永久、不受缓存 LRU 驱逐);
//! 播放解析(见 [`crate::resolve`])直接探测该目录命中,**无需再复制进缓存**。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, eyre};
use futures_util::StreamExt;
use mineral_channel_core::MusicChannel;
use mineral_model::{AudioFormat, BitRate, MediaUrl, PlayUrl, Song};
use mineral_protocol::{DownloadProgress, DownloadTarget};
use parking_lot::Mutex;
use tokio::io::AsyncWriteExt;

use crate::media_cache::library_relpath;
use crate::player::PlayerCore;

/// 速度刷新节流间隔:每隔这么久才重算一次瞬时速度 + 写进度态。
const SPEED_TICK: Duration = Duration::from_millis(150);

/// 一首下载的结局(`Err` 另表失败):区分「真正下载」与「已存在跳过」,供完成提示分流统计。
pub(crate) enum DownloadOutcome {
    /// 真正流式下载并永久导出。
    Downloaded,

    /// 目标文件已存在,幂等跳过。
    Skipped,
}

/// 下载音质。后续接 config 时改读配置(与播放音质各自独立)。
const DOWNLOAD_QUALITY: BitRate = BitRate::Lossless;

/// 解析下载环境:HTTP client(整段 GET 用)+ 永久导出根目录(`~/Music/mineral`)。
/// 任一不可用时对应项为 `None`(下载整体降级为「不可用」,只 warn 不阻断启动)。
///
/// # Return:
///   `(http, music_dir)`,各自失败为 `None`。
pub(crate) fn open_env() -> (Option<reqwest::Client>, Option<PathBuf>) {
    let http = reqwest::Client::builder().build().ok();
    if http.is_none() {
        mineral_log::warn!(target: "download", "HTTP client 构建失败,下载不可用");
    }
    let music_dir = match mineral_paths::music_export_dir() {
        Ok(d) => Some(d),
        Err(e) => {
            mineral_log::warn!(target: "download", error = mineral_log::chain(&e), "解析音乐导出目录失败,下载不可用");
            None
        }
    };
    (http, music_dir)
}

/// 下载一首歌:**流式** GET(边下边写、边算速度写进度)→ 永久导出。
/// 该歌该音质已在导出库(文件系统即真相)则跳过,连直链都不取(按文件存在幂等)。
///
/// 不再复制进缓存:导出目录本身即播放解析的命中源(见 [`crate::resolve`]),复制只会徒增
/// 双份存储、并让播放走 LRU 副本而非永久文件。
///
/// # Params:
///   - `channel`: 该曲来源的 channel(取直链)
///   - `http`: 复用的 HTTP client
///   - `music_dir`: 永久导出根目录(如 `~/Music/mineral`)
///   - `song`: 要下载的歌
///   - `quality`: 下载音质
///   - `progress`: 下载进度共享态(本函数实时写 `bytes_done`/`bytes_total`/`speed_bps`)
///
/// # Return:
///   下载成功 → `Ok(Downloaded)`;已下载 → `Ok(Skipped)`;取链 / 网络 / 写盘失败 → `Err`。
pub(crate) async fn download_song(
    channel: &dyn MusicChannel,
    http: &reqwest::Client,
    music_dir: &Path,
    song: &Song,
    quality: BitRate,
    progress: &Arc<Mutex<DownloadProgress>>,
) -> color_eyre::Result<DownloadOutcome> {
    // 1. 幂等:该歌该音质已在导出库 → 跳过(文件系统即真相,按 <album>/<title>.* 反查)。
    if crate::resolve::probe_export(music_dir, song, quality).is_some() {
        mineral_log::debug!(target: "download", song_id = song.id.as_str(), "已下载,跳过");
        return Ok(DownloadOutcome::Skipped);
    }

    // 2. 取直链 + 实际格式(格式定扩展名,得在算导出路径前拿到)。
    let urls = channel
        .song_urls(std::slice::from_ref(&song.id), quality)
        .await
        .map_err(|e| eyre!("song_urls: {e}"))?;
    let play_url = urls
        .into_iter()
        .next()
        .ok_or_else(|| eyre!("无可用播放 URL: {}", song.id.qualified()))?;
    let (subdir, file_name) = library_relpath(song, quality, &play_url.format);
    // 命名即身份:不做 ` (N)` 去重——同名直接落同一路径(本曲重下已被上面的幂等挡住;同源同专辑
    // 同名的另一首歌会与之共用一个文件,概率极低,换来「文件系统即唯一真相」)。
    let export = music_dir.join(&subdir).join(&file_name);
    let remote = match play_url.url {
        MediaUrl::Remote(u) => u,
        MediaUrl::Local(p) => {
            return Err(eyre!("song_urls 返回本地路径,无需下载: {}", p.display()));
        }
    };

    // 3. 流式下载到 `<export>.part-dl`,边写边更新进度 / 速度。
    if let Some(parent) = export.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .wrap_err_with(|| format!("创建导出目录失败 {}", parent.display()))?;
    }
    let part = export.with_extension("part-dl");
    stream_to_file(http, remote, &part, progress).await?;

    // 4. 完成 → rename 为正式导出(永久)。
    tokio::fs::rename(&part, &export)
        .await
        .wrap_err_with(|| format!("rename 导出失败 {}", export.display()))?;
    mineral_log::info!(target: "download", song_id = song.id.as_str(), path = %export.display(), "下载完成");
    Ok(DownloadOutcome::Downloaded)
}

/// 流式把 `url` 下载到 `dst`,逐 chunk 写盘并按 [`SPEED_TICK`] 节流更新 `progress` 的
/// `bytes_done` / `bytes_total` / 平滑 `speed_bps`(整数 EMA)。
///
/// # Params:
///   - `http`: HTTP client
///   - `url`: 直链
///   - `dst`: 目标(临时)文件
///   - `progress`: 进度共享态
///
/// # Return:
///   下完返回 `Ok(())`。
async fn stream_to_file(
    http: &reqwest::Client,
    url: url::Url,
    dst: &Path,
    progress: &Arc<Mutex<DownloadProgress>>,
) -> color_eyre::Result<()> {
    let resp = http
        .get(url)
        .send()
        .await
        .wrap_err("下载请求失败")?
        .error_for_status()
        .wrap_err("下载响应非 2xx")?;
    let total = resp.content_length().unwrap_or(0);
    {
        let mut p = progress.lock();
        p.bytes_done = 0;
        p.bytes_total = total;
        p.speed_bps = 0;
    }
    let mut file = tokio::fs::File::create(dst)
        .await
        .wrap_err_with(|| format!("创建下载临时文件失败 {}", dst.display()))?;
    let mut stream = resp.bytes_stream();
    let mut done = 0u64;
    let mut ema = 0u64;
    let mut win_start = Instant::now();
    let mut win_bytes = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.wrap_err("下载流读取失败")?;
        file.write_all(&chunk)
            .await
            .wrap_err("写下载临时文件失败")?;
        done = done.saturating_add(u64::try_from(chunk.len()).unwrap_or(0));
        let dt = win_start.elapsed();
        if dt >= SPEED_TICK {
            let dt_ms = u64::try_from(dt.as_millis()).unwrap_or(u64::MAX).max(1);
            let inst = done.saturating_sub(win_bytes).saturating_mul(1000) / dt_ms;
            // 整数 EMA:0.6 旧 + 0.4 新,首样本直接采用。
            ema = if ema == 0 {
                inst
            } else {
                (ema.saturating_mul(3) + inst.saturating_mul(2)) / 5
            };
            let mut p = progress.lock();
            p.bytes_done = done;
            p.speed_bps = ema;
            win_start = Instant::now();
            win_bytes = done;
        }
    }
    file.flush().await.wrap_err("flush 下载临时文件失败")?;
    let mut p = progress.lock();
    p.bytes_done = done;
    if p.bytes_total == 0 {
        p.bytes_total = done;
    }
    Ok(())
}

/// 一首正在 capture(边播边落盘)的曲的上下文:播完 / 下完后据此入缓存,中途打断则删 `path`。
pub(crate) struct Capturing {
    /// 在播的歌(组库路径取 source / album / title)。
    pub(crate) song: Song,

    /// 入库音质(与播放请求一致,决定 index 键 / 目录)。
    pub(crate) quality: BitRate,

    /// 实际音频格式(决定扩展名)。
    pub(crate) format: AudioFormat,

    /// capture 落盘临时路径(engine 正往这写)。
    pub(crate) path: PathBuf,
}

/// 以远端 URL 起播,并(缓存可用时)把下载字节 capture 到临时文件、登记 [`Capturing`]
/// 供下完 / 播完入缓存;缓存禁用时退回普通播放。
///
/// # Params:
///   - `player`: 播放核心(取 audio / media_cache、登记 capturing)
///   - `song`: 在播的歌
///   - `pu`: 该歌的播放 URL(取 `url` 起播、`format` 定扩展名)
///   - `quality`: 入库音质(与请求一致)
pub(crate) fn play_capturing(player: &PlayerCore, song: &Song, pu: &PlayUrl, quality: BitRate) {
    match player.media_cache().capture_path(&song.id, quality) {
        Some(path) => {
            player.audio().play_capturing(pu.url.clone(), path.clone());
            player.set_capturing(Capturing {
                song: song.clone(),
                quality,
                format: pu.format.clone(),
                path,
            });
        }
        None => player.audio().play(pu.url.clone()),
    }
}

/// 把一首已下完的 capture 文件后台收编进缓存(spawn_blocking,不阻塞 loop)。
/// 文件缺失 / 空(下载未完成)→ 不入缓存并删残件。
///
/// # Params:
///   - `player`: 播放核心(取 media_cache)
///   - `cap`: 该曲的 capture 上下文
pub(crate) fn spawn_harvest(player: &PlayerCore, cap: Capturing) {
    let cache = Arc::clone(player.media_cache());
    // async task(非 spawn_blocking):put_played 要 await DB 写穿透;入库内部的大拷贝由它自己
    // 再下沉到 spawn_blocking。metadata 是一次快速 stat,async 里直接调可接受。
    tokio::spawn(async move {
        match std::fs::metadata(&cap.path) {
            Ok(m) if m.len() > 0 => {
                if let Err(e) = cache
                    .put_played(&cap.song, cap.quality, &cap.format, &cap.path)
                    .await
                {
                    mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "音频入缓存失败");
                }
            }
            _ => {
                mineral_log::debug!(target: "player", "capture 文件缺失/空,不入缓存");
                drop(std::fs::remove_file(&cap.path));
            }
        }
    });
}

/// 下载 worker:**单线串行**消费队列,把所有目标聚合进**同一进度会话**(`done`/`total`
/// 按歌曲数累加,如 2/21 加一个 3 首歌单 → 2/24)。`pending` 归 0(本批是最后一个)即收尾。
///
/// # Params:
///   - `player`: 播放核心
///   - `rx`: 下载目标接收端
///   - `pending`: 与 `download()` 共享的未完成批数(归 0 → 会话结束)
pub(crate) async fn worker(
    player: PlayerCore,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<DownloadTarget>,
    pending: Arc<std::sync::atomic::AtomicUsize>,
) {
    while let Some(target) = rx.recv().await {
        process_target(&player, target).await;
        // 本批处理完。pending 归 0(无后续)→ 会话收尾:出完成提示 + 复位进度。
        if pending.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 {
            finalize(&player);
        }
    }
}

/// 处理一个下载目标:解析歌曲(歌单此时才知数 → 累加 `total`),逐首串行下载、累加 `done`/成败。
///
/// # Params:
///   - `player`: 播放核心
///   - `target`: 下载目标
async fn process_target(player: &PlayerCore, target: DownloadTarget) {
    let (Some(http), Some(music_dir)) = (player.http(), player.music_dir()) else {
        player.push_notice("下载不可用(无 HTTP client / 音乐目录)".to_owned());
        return;
    };
    let songs = match collect_songs(player, &target).await {
        Ok(s) => s,
        Err(text) => {
            player.push_notice(text);
            return;
        }
    };
    // 单曲已在 `download()` 入队时计过 total;歌单数现在才知,补加。
    if matches!(target, DownloadTarget::Playlist(_)) {
        player.progress_handle().lock().total += songs.len();
    }
    let Some(channel) = songs
        .first()
        .and_then(|s| player.channel_for(s.source()))
        .cloned()
    else {
        return;
    };
    for song in &songs {
        {
            let mut p = player.progress_handle().lock();
            p.bytes_done = 0;
            p.bytes_total = 0;
            p.speed_bps = 0;
        }
        let outcome = download_song(
            channel.as_ref(),
            http,
            music_dir,
            song,
            DOWNLOAD_QUALITY,
            player.progress_handle(),
        )
        .await;
        let mut p = player.progress_handle().lock();
        p.done += 1;
        match outcome {
            Ok(DownloadOutcome::Downloaded) => p.last_ok += 1,
            Ok(DownloadOutcome::Skipped) => p.last_skip += 1,
            Err(e) => {
                drop(p);
                mineral_log::warn!(target: "download", song_id = song.id.as_str(), error = mineral_log::chain(&e), "下载失败");
                player.progress_handle().lock().last_fail += 1;
            }
        }
    }
}

/// 会话收尾:`result_seq` +1(client 据其增长出一次完成提示),复位进度态、`active=false`;
/// 保留 `last_ok`/`last_fail`/`result_seq` 供 client 读取(下次会话开始时由 `download()` 复位)。
///
/// # Params:
///   - `player`: 播放核心
fn finalize(player: &PlayerCore) {
    let mut p = player.progress_handle().lock();
    p.result_seq = p.result_seq.wrapping_add(1);
    p.active = false;
    p.done = 0;
    p.total = 0;
    p.bytes_done = 0;
    p.bytes_total = 0;
    p.speed_bps = 0;
    p.queued = 0;
}

/// 把下载目标解析成待下歌曲列表:单曲直接 1 首;歌单 server 端拉 tracks。
///
/// # Params:
///   - `player`: 播放核心(歌单拉 tracks 用 channel)
///   - `target`: 下载目标
///
/// # Return:
///   待下歌曲;失败返回 `Err(给用户看的提示文本)`。
async fn collect_songs(player: &PlayerCore, target: &DownloadTarget) -> Result<Vec<Song>, String> {
    match target {
        DownloadTarget::Song(song) => Ok(vec![song.as_ref().clone()]),
        DownloadTarget::Playlist(id) => {
            let channel = player
                .channel_for(id.namespace())
                .cloned()
                .ok_or_else(|| "下载失败: 无来源 channel".to_owned())?;
            channel.songs_in_playlist(id).await.map_err(|e| {
                mineral_log::warn!(target: "download", error = mineral_log::chain(&e), "拉歌单曲目失败");
                "下载失败: 拉歌单曲目失败".to_owned()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_model::{AlbumId, AlbumRef, BitRate, Song, SongId, SourceKind};
    use mineral_persist::ServerStore;
    use mineral_protocol::DownloadProgress;
    use mineral_test::mock::{UrlChannel, serve_once};
    use parking_lot::Mutex;

    use super::{DownloadOutcome, download_song};
    use crate::media_cache::MediaCache;
    use crate::resolve::probe_export;

    /// 一首带专辑的测试歌曲。
    fn song() -> Song {
        Song {
            id: SongId::new(SourceKind::NETEASE, "1"),
            name: "t".to_owned(),
            artists: Vec::new(),
            album: Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "0"),
                name: "A".to_owned(),
            }),
            duration_ms: 0,
            cover_url: None,
            source_url: None,
        }
    }

    /// 回归:`download_song` 下完后**只**落永久导出目录,**不应**复制进 audio cache
    /// (否则双份存储,且播放会走 LRU 缓存副本而非永久下载文件)。带 `fill_cache` 时此断言变红。
    #[tokio::test]
    async fn download_does_not_populate_cache() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let media_cache =
            MediaCache::open(&persist, dir.path().join("cache"), 1_000_000_000).await?;
        let music_dir = dir.path().join("music");

        let url = serve_once(b"FAKEFLACDATA".to_vec()).await?;
        let channel = UrlChannel { url };
        let http = reqwest::Client::new();
        let progress = Arc::new(Mutex::new(DownloadProgress::default()));
        let s = song();

        let outcome = download_song(
            &channel,
            &http,
            &music_dir,
            &s,
            BitRate::Lossless,
            &progress,
        )
        .await?;
        assert!(matches!(outcome, DownloadOutcome::Downloaded), "应真正下载");
        assert!(
            probe_export(&music_dir, &s, BitRate::Lossless).is_some(),
            "永久下载文件应已落盘"
        );
        assert!(
            media_cache.get(&s.id, BitRate::Lossless).is_none(),
            "下载不应填充 audio cache(避免双份 + 播放走缓存副本)"
        );
        Ok(())
    }
}
