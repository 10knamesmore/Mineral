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

/// 一首下载的结局(`Err` 另表失败):区分「真正下载」与「已存在跳过」,供完成提示分流统计。
pub(crate) enum DownloadOutcome {
    /// 真正流式下载并永久导出(完成事件下发用)。
    Downloaded {
        /// 落盘路径。
        path: PathBuf,

        /// 实际下载音质(hook 改写后的有效值)。
        quality: mineral_model::BitRate,

        /// 容器格式(channel 实际提供;拿不到为 `Other("")`)。
        format: mineral_model::AudioFormat,
    },

    /// 目标文件已存在,幂等跳过(**不**触发完成事件)。
    Skipped,
}

/// 解析下载环境:HTTP client(整段 GET 用)+ 永久导出根目录。
/// 任一不可用时对应项为 `None`(下载整体降级为「不可用」,只 warn 不阻断启动)。
///
/// 导出目录优先级:config(`download.dir`)> 平台默认(`~/Music/mineral`)。
/// config.lua 是唯一用户真相源,不设环境变量逃逸口。
///
/// # Params:
///   - `config_dir`: 配置的下载目录(`download.dir`;`None` = 未配置)
///
/// # Return:
///   `(http, music_dir)`,各自失败为 `None`。
pub(crate) fn open_env(config_dir: Option<&Path>) -> (Option<reqwest::Client>, Option<PathBuf>) {
    let http = reqwest::Client::builder().build().ok();
    if http.is_none() {
        mineral_log::warn!(target: "download", "HTTP client 构建失败,下载不可用");
    }
    let music_dir = if let Some(d) = config_dir {
        Some(d.to_path_buf())
    } else {
        match mineral_paths::music_export_dir() {
            Ok(d) => Some(d),
            Err(e) => {
                mineral_log::warn!(target: "download", error = mineral_log::chain(&e), "解析音乐导出目录失败,下载不可用");
                None
            }
        }
    };
    (http, music_dir)
}

/// 下载环境:HTTP client + 导出根目录 + 脚本拦截门
/// (`process_target` 从 [`PlayerCore`] 取齐,单测各自注入)。
#[derive(Clone, Copy)]
pub(crate) struct DownloadEnv<'a> {
    /// 复用的 HTTP client。
    pub(crate) http: &'a reqwest::Client,

    /// 永久导出根目录(如 `~/Music/mineral`)。
    pub(crate) music_dir: &'a Path,

    /// 脚本拦截门(`before_download`;无脚本恒放行)。
    pub(crate) hooks: &'a crate::hook_bridge::HookGate,
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
///   - `env`: 下载环境(HTTP client + 导出根目录 + 脚本拦截门)
///   - `progress`: 下载进度共享态(本函数实时写 `bytes_done`/`bytes_total`/`speed_bps`)
///   - `speed_tick`: 测速刷新节流间隔(配置 `daemon.download_speed_tick_ms`)
///
/// # Return:
///   下载成功 → `Ok(Downloaded)`;已下载 / 脚本跳过 → `Ok(Skipped)`;
///   取链 / 网络 / 写盘失败 → `Err`。
pub(crate) async fn download_song(
    channel: &dyn MusicChannel,
    env: &DownloadEnv<'_>,
    song: &Song,
    quality: BitRate,
    progress: &Arc<Mutex<DownloadProgress>>,
    speed_tick: Duration,
) -> color_eyre::Result<DownloadOutcome> {
    let DownloadEnv {
        http,
        music_dir,
        hooks,
    } = *env;
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
    let mut play_url = urls
        .into_iter()
        .next()
        .ok_or_else(|| eyre!("无可用播放 URL: {}", song.id.qualified()))?;

    // 2.5 脚本拦截:before_download(取链后、算导出路径 / 写盘前)。
    let mut quality = quality;
    match hooks.before_download(song, &play_url).await {
        mineral_script::HookDecision::Continue => {}
        mineral_script::HookDecision::Rewrite(spec) => {
            if let Some(url) = spec.new_url() {
                play_url.url = url.clone();
            }
            if let Some(new_quality) = spec.new_quality() {
                play_url.quality = new_quality;
                // 导出路径按音质标注,改写音质要一并体现。
                quality = new_quality;
            }
            mineral_log::info!(
                target: "script",
                song_id = song.id.as_str(),
                url = %play_url.url,
                "before_download 改写下载直链"
            );
        }
        mineral_script::HookDecision::Skip { reason } => {
            mineral_log::info!(
                target: "script",
                song_id = song.id.as_str(),
                reason,
                "before_download 跳过本曲"
            );
            return Ok(DownloadOutcome::Skipped);
        }
    }
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
    stream_to_file(
        http,
        remote,
        &part,
        progress,
        speed_tick,
        &play_url.stream_headers,
    )
    .await?;

    // 4. 完成 → rename 为正式导出(永久)。
    tokio::fs::rename(&part, &export)
        .await
        .wrap_err_with(|| format!("rename 导出失败 {}", export.display()))?;
    mineral_log::info!(target: "download", song_id = song.id.as_str(), path = %export.display(), "下载完成");
    Ok(DownloadOutcome::Downloaded {
        path: export,
        quality,
        format: play_url.format.clone(),
    })
}

/// 流式把 `url` 下载到 `dst`,逐 chunk 写盘并按 `speed_tick` 节流更新 `progress` 的
/// `bytes_done` / `bytes_total` / 平滑 `speed_bps`(整数 EMA)。
///
/// # Params:
///   - `http`: HTTP client
///   - `url`: 直链
///   - `dst`: 目标(临时)文件
///   - `progress`: 进度共享态
///   - `speed_tick`: 测速刷新节流间隔(配置 `daemon.download_speed_tick_ms`)
///
/// # Return:
///   下完返回 `Ok(())`。
async fn stream_to_file(
    http: &reqwest::Client,
    url: url::Url,
    dst: &Path,
    progress: &Arc<Mutex<DownloadProgress>>,
    speed_tick: Duration,
    headers: &[(String, String)],
) -> color_eyre::Result<()> {
    use reqwest::header::{HeaderName, HeaderValue};

    let mut req = http.get(url);
    // 取流附加头(如 B站 baseUrl 下载需 `Referer`);非法头跳过并 warn,不掀掉整条下载。
    for (name, value) in headers {
        match (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            (Ok(n), Ok(v)) => req = req.header(n, v),
            _ => mineral_log::warn!(target: "download", header = %name, "跳过非法取流头"),
        }
    }
    let resp = req
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
        if dt >= speed_tick {
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
            player.audio().play_capturing(
                pu.url.clone(),
                pu.stream_headers.clone(),
                path.clone(),
                pu.layout,
            );
            player.set_capturing(Capturing {
                song: song.clone(),
                quality,
                format: pu.format.clone(),
                path,
            });
        }
        None => player
            .audio()
            .play(pu.url.clone(), pu.stream_headers.clone(), pu.layout),
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
        player.notify().toast(
            mineral_protocol::ToastKind::Warn,
            "下载不可用(无 HTTP client / 音乐目录)".to_owned(),
        );
        return;
    };
    let songs = match collect_songs(player, &target).await {
        Ok(s) => s,
        Err(text) => {
            player
                .notify()
                .toast(mineral_protocol::ToastKind::Warn, text);
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
    let hooks = player.hook_gate();
    let env = DownloadEnv {
        http,
        music_dir,
        hooks: &hooks,
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
            &env,
            song,
            player.download_quality(),
            player.progress_handle(),
            player.download_speed_tick(),
        )
        .await;
        let mut p = player.progress_handle().lock();
        p.done += 1;
        match outcome {
            Ok(DownloadOutcome::Downloaded {
                path,
                quality,
                format,
            }) => {
                p.last_ok += 1;
                drop(p);
                player
                    .notify()
                    .download_completed(song, &path, quality, &format);
                p = player.progress_handle().lock();
            }
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
            channel
                .playlist_detail(id)
                .await
                .map(|pl| pl.songs)
                .map_err(|e| {
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

    use std::time::Duration;

    use super::{DownloadOutcome, download_song};
    use crate::media_cache::MediaCache;
    use crate::resolve::probe_export;

    /// 一首带专辑的测试歌曲。
    fn song() -> Song {
        Song::builder()
            .id(SongId::new(SourceKind::NETEASE, "1"))
            .name("t".to_owned())
            .album(Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "0"),
                name: "A".to_owned(),
            }))
            .build()
    }

    /// 回归:`download_song` 下完后**只**落永久导出目录,**不应**复制进 audio cache
    /// (否则双份存储,且播放会走 LRU 缓存副本而非永久下载文件)。带 `fill_cache` 时此断言变红。
    // multi_thread:走真实 TCP I/O(serve_once 的 server 任务 + reqwest client),单线程
    // runtime 下两者靠协作调度,重负载时偶发连接重置 → flaky;给 server 独立 worker 线程。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
            &super::DownloadEnv {
                http: &http,
                music_dir: &music_dir,
                hooks: &crate::hook_bridge::HookGate::disabled(),
            },
            &s,
            BitRate::Lossless,
            &progress,
            /*speed_tick*/ Duration::from_millis(150),
        )
        .await?;
        assert!(
            matches!(outcome, DownloadOutcome::Downloaded { .. }),
            "应真正下载"
        );
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

    /// eval 给定脚本并把投递句柄包成拦截门(download hook 测试用)。
    /// 返回的 runtime 须由调用方持有(drop 即停脚本线程)。
    fn script_gate(
        script: &str,
    ) -> color_eyre::Result<(mineral_script::ScriptRuntime, crate::hook_bridge::HookGate)> {
        use mineral_script::{ScriptHost, ScriptRuntime, ScriptSender, install_api};
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = mineral_script::mlua::Lua::new();
        install_api(&lua, &host)?;
        lua.load(script).exec()?;
        let sender = ScriptSender::detached();
        let watchdog = mineral_script::WatchdogConfig::builder()
            .instruction_interval(10_000)
            .soft_wall(Duration::from_millis(200))
            .hard_wall(Duration::from_secs(1))
            .build();
        let runtime = ScriptRuntime::spawn(lua, host, watchdog, &sender)?;
        let gate = crate::hook_bridge::HookGate::with_sender(sender, Duration::from_secs(5));
        Ok((runtime, gate))
    }

    /// before_download 跳过:hook 返回 {skip=...} → Skipped,不落盘、不发请求。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn download_skipped_by_hook() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let music_dir = dir.path().join("music");
        // 直链指向无人监听的端口:skip 生效就不会有任何网络请求。
        let channel = UrlChannel {
            url: "http://127.0.0.1:9/dead.flac".parse()?,
        };
        let (runtime, gate) = script_gate(
            r#"
            mineral.hook("before_download", function(ctx)
                return { skip = "脚本拒绝" }
            end)
            "#,
        )?;
        let outcome = download_song(
            &channel,
            &super::DownloadEnv {
                http: &reqwest::Client::new(),
                music_dir: &music_dir,
                hooks: &gate,
            },
            &song(),
            BitRate::Lossless,
            &Arc::new(Mutex::new(DownloadProgress::default())),
            /*speed_tick*/ Duration::from_millis(150),
        )
        .await?;
        assert!(
            matches!(outcome, DownloadOutcome::Skipped),
            "hook 跳过应记 Skipped"
        );
        assert!(
            probe_export(&music_dir, &song(), BitRate::Lossless).is_none(),
            "跳过不应落盘"
        );
        drop(runtime);
        Ok(())
    }

    /// before_download 改写:原直链死地址,hook 改写到真 server + 降音质 →
    /// 下载成功且导出路径按改写后音质标注。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn download_rewritten_by_hook() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let music_dir = dir.path().join("music");
        let live = serve_once(b"FAKEFLACDATA".to_vec()).await?;
        // 原直链是死地址:下载成功本身就证明改写生效。
        let channel = UrlChannel {
            url: "http://127.0.0.1:9/dead.flac".parse()?,
        };
        let (runtime, gate) = script_gate(&format!(
            r#"
            mineral.hook("before_download", function(ctx)
                return {{ url = "{live}", quality = "standard" }}
            end)
            "#
        ))?;
        let outcome = download_song(
            &channel,
            &super::DownloadEnv {
                http: &reqwest::Client::new(),
                music_dir: &music_dir,
                hooks: &gate,
            },
            &song(),
            BitRate::Lossless,
            &Arc::new(Mutex::new(DownloadProgress::default())),
            /*speed_tick*/ Duration::from_millis(150),
        )
        .await?;
        assert!(
            matches!(outcome, DownloadOutcome::Downloaded { .. }),
            "改写到活地址应下载成功"
        );
        assert!(
            probe_export(&music_dir, &song(), BitRate::Standard).is_some(),
            "导出路径应按改写后的音质(standard)标注"
        );
        drop(runtime);
        Ok(())
    }

    /// `stream_to_file` 把 `PlayUrl.stream_headers` 注入下载请求:B站 baseUrl 下载必须带
    /// `Referer`,否则 403。起一个记录请求头的本地 server,带 Referer 下载,断言 server 收到该头。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stream_to_file_injects_stream_headers() -> color_eyre::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let recorded = Arc::new(Mutex::new(None::<String>));
        let rec = Arc::clone(&recorded);
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = buf
                    .get(..n)
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_default();
                *rec.lock() = Some(req);
                let body = b"FAKEDATA";
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                drop(sock.write_all(head.as_bytes()).await);
                drop(sock.write_all(body).await);
                drop(sock.shutdown().await);
            }
        });

        let dir = tempfile::tempdir()?;
        let dst = dir.path().join("out.part-dl");
        let http = reqwest::Client::new();
        let progress = Arc::new(Mutex::new(DownloadProgress::default()));
        let url = url::Url::parse(&format!("http://{addr}/a.mp3"))?;
        super::stream_to_file(
            &http,
            url,
            &dst,
            &progress,
            Duration::from_millis(100),
            &[("Referer".to_owned(), "https://www.bilibili.com".to_owned())],
        )
        .await?;

        let raw = recorded.lock().clone().unwrap_or_default();
        // HTTP header 名大小写不敏感(hyper 发小写),按名匹配、value 保原样。
        let referer = raw.lines().find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.trim()
                    .eq_ignore_ascii_case("referer")
                    .then(|| value.trim().to_owned())
            })
        });
        assert_eq!(
            referer.as_deref(),
            Some("https://www.bilibili.com"),
            "stream_to_file 应把 headers 注入下载请求;实际收到:\n{raw}"
        );
        Ok(())
    }
}
