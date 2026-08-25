//! Provider-backed download/export draining decoder-ready opened media to permanent storage.
//!
//! 这是可复用单元——键位下载单曲 / 歌单批量等场景都调 [`download_song`]:
//! 导出落 `<music_dir>/<source>/<quality>/<album>/<title>.<ext>`(永久、不受缓存 LRU 驱逐);
//! 播放解析(见 [`crate::resolve`])直接探测该目录命中,不复制进缓存。provider resolve/open
//! 产生 decoder-ready encoded media，export 顺序写入 preparation 后的 bytes。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, eyre};
use mineral_model::{AudioFormat, BitRate, Song};
use mineral_playback::{OpenOptions, PlaybackRegistry, PlaybackRequest};
use mineral_protocol::{DownloadProgress, DownloadTarget};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::media_cache::library_relpath;
use crate::player::PlayerCore;

/// 一首下载的结局(`Err` 另表失败):区分「真正下载」与两类跳过(幂等 / 脚本否决),
/// 供完成提示分流统计。hook 裁决随结局携带——埋点据此落 downloads.hooked 列,脚本
/// veto 与幂等跳过在库里才分得开。
pub(crate) enum DownloadOutcome {
    /// Prepared media was permanently exported.
    Downloaded {
        /// 落盘路径。
        path: PathBuf,

        /// 实际下载音质(hook 改写后的有效值)。
        quality: mineral_model::BitRate,

        /// Provider 最终交付的容器格式;拿不到为 `None`。
        format: Option<mineral_model::AudioFormat>,

        /// before_download 的裁决(未改写为 `None`、改写为 `Rewrite`)。
        hooked: mineral_stats::DownloadHook,
    },

    /// 未下载而跳过(**不**触发完成事件);成因见 [`SkipCause`]。
    Skipped {
        /// 跳过成因。
        cause: SkipCause,
    },
}

/// 一次下载跳过的成因(埋点 hooked 列据此分流)。
pub(crate) enum SkipCause {
    /// 目标文件已存在,幂等跳过(hooked=none)。
    AlreadyExists,

    /// before_download 返回 Skip,脚本否决(hooked=skip)。
    HookVeto,
}

/// 解析下载环境:永久导出根目录。不可用时为 `None`(下载整体降级为「不可用」,
/// 只 warn 不阻断启动)。
///
/// 导出目录优先级:config(`download.dir`)> 平台默认(`~/Music/mineral`)。
/// config.lua 是唯一用户真相源,不设环境变量逃逸口。
///
/// # Params:
///   - `config_dir`: 配置的下载目录(`download.dir`;`None` = 未配置)
///
/// # Return:
///   导出根目录;解析失败为 `None`。
pub(crate) fn open_env(config_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(d) = config_dir {
        Some(d.to_path_buf())
    } else {
        match mineral_paths::music_export_dir() {
            Ok(d) => Some(d),
            Err(e) => {
                mineral_log::warn!(target: "download", error = mineral_log::chain(&e), "解析音乐导出目录失败,下载不可用");
                None
            }
        }
    }
}

/// 下载环境:导出根目录 + 脚本拦截门
/// (`process_target` 从 [`PlayerCore`] 取齐,单测各自注入)。
#[derive(Clone, Copy)]
pub(crate) struct DownloadEnv<'a> {
    /// 永久导出根目录(如 `~/Music/mineral`)。
    pub(crate) music_dir: &'a Path,

    /// 脚本拦截门(`before_download`;无脚本恒放行)。
    pub(crate) hooks: &'a crate::hook_bridge::HookGate,
}

/// Resolves and opens one prepared playback, then drains its encoded bytes to permanent storage.
/// 该歌该音质已在导出库(文件系统即真相)则跳过,不调用 provider。
///
/// 导出目录本身即播放解析的命中源(见 [`crate::resolve`]),不复制进缓存——复制只会
/// 徒增双份存储、并让播放走 LRU 副本而非永久文件。
///
/// # Params:
///   - `playback`: Playback provider registry.
///   - `song`: 要下载的歌
///   - `quality`: 下载音质
///   - `env`: 下载环境(导出根目录 + 脚本拦截门)
///   - `progress`: 下载进度共享态(本函数实时写 `bytes_done`/`bytes_total`/`speed_bps`)
///   - `speed_tick`: 测速刷新节流间隔(配置 `daemon.download_speed_tick_ms`)
///
/// # Return:
///   下载成功 → `Ok(Downloaded)`;已下载 / 脚本跳过 → `Ok(Skipped)`;
///   provider resolve/open、reader 或写盘失败 → `Err`。
pub(crate) async fn download_song(
    playback: &PlaybackRegistry,
    env: &DownloadEnv<'_>,
    song: &Song,
    quality: BitRate,
    progress: &Arc<Mutex<DownloadProgress>>,
    speed_tick: Duration,
) -> color_eyre::Result<DownloadOutcome> {
    let DownloadEnv { music_dir, hooks } = *env;
    // 1. 幂等:该歌该音质已在导出库 → 跳过(文件系统即真相,按 <album>/<title>.* 反查)。
    if crate::resolve::probe_export(music_dir, song, quality).is_some() {
        mineral_log::debug!(target: "download", song_id = song.id.as_str(), "已下载,跳过");
        return Ok(DownloadOutcome::Skipped {
            cause: SkipCause::AlreadyExists,
        });
    }

    let provider = playback
        .get(song.source())
        .ok_or_else(|| eyre!("no playback provider for {:?}", song.source()))?;
    let cancellation = CancellationToken::new();
    let mut prepared = provider
        .resolve(
            PlaybackRequest::new(song.id.clone(), quality),
            cancellation.child_token(),
        )
        .await?;

    let mut hooked = mineral_stats::DownloadHook::None;
    match hooks.before_download(song, prepared.direct_media()).await {
        mineral_script::HookDecision::Continue => {}
        mineral_script::HookDecision::Rewrite(spec) => {
            hooked = mineral_stats::DownloadHook::Rewrite;
            let original_direct = prepared.direct_media().cloned();
            prepared =
                crate::hook_bridge::rewrite_prepared(&song.id, original_direct.as_ref(), &spec)
                    .ok_or_else(|| eyre!("download rewrite has no direct replacement"))?;
        }
        mineral_script::HookDecision::Skip { reason } => {
            mineral_log::info!(
                target: "script",
                song_id = song.id.as_str(),
                reason,
                "before_download 跳过本曲"
            );
            return Ok(DownloadOutcome::Skipped {
                cause: SkipCause::HookVeto,
            });
        }
    }
    let opened = prepared
        .open(OpenOptions::new(
            cancellation.child_token(),
            /*prefetch_bytes*/ 0,
        ))
        .await?;
    let quality = opened.info().quality;
    let format = opened.info().format.clone();
    let byte_len = opened.byte_len();
    let (subdir, file_name) = library_relpath(song, quality, format.as_ref());
    // 命名即身份:不做 ` (N)` 去重——同名直接落同一路径(本曲重下已被上面的幂等挡住;同源同专辑
    // 同名的另一首歌会与之共用一个文件,概率极低,换来「文件系统即唯一真相」)。
    let export = music_dir.join(&subdir).join(&file_name);
    if let Some(parent) = export.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .wrap_err_with(|| format!("创建导出目录失败 {}", parent.display()))?;
    }
    let part = export.with_extension("part-dl");
    let reader = opened.into_reader();
    let progress_handle = Arc::clone(progress);
    let part_for_write = part.clone();
    tokio::task::spawn_blocking(move || {
        drain_opened(
            reader,
            &part_for_write,
            byte_len,
            &progress_handle,
            speed_tick,
        )
    })
    .await
    .map_err(|error| eyre!("download writer task: {error}"))??;
    {
        // 无 Content-Length 时 total 从未上报,下完以实际字节数补满。
        let mut p = progress.lock();
        if p.bytes_total == 0 {
            p.bytes_total = p.bytes_done;
        }
    }

    // 4. 完成 → rename 为正式导出(永久)。
    tokio::fs::rename(&part, &export)
        .await
        .wrap_err_with(|| format!("rename 导出失败 {}", export.display()))?;
    mineral_log::info!(target: "download", song_id = song.id.as_str(), path = %export.display(), "下载完成");
    Ok(DownloadOutcome::Downloaded {
        path: export,
        quality,
        format,
        hooked,
    })
}

/// 一首正在 capture(边播边落盘)的曲的上下文:播完 / 下完后据此入缓存,中途打断则删 `path`。
pub(crate) struct Capturing {
    /// 在播的歌(组库路径取 source / album / title)。
    pub(crate) song: Song,

    /// 入库音质(与播放请求一致,决定 index 键 / 目录)。
    pub(crate) quality: BitRate,

    /// 实际音频格式(决定扩展名;未知按音质兜底)。
    pub(crate) format: Option<AudioFormat>,

    /// capture 落盘临时路径(engine 正往这写)。
    pub(crate) path: PathBuf,
}

/// Drains one synchronous opened-media reader into a permanent part file.
fn drain_opened(
    mut reader: Box<dyn mineral_playback::MediaReader>,
    part: &Path,
    byte_len: Option<u64>,
    progress: &Arc<Mutex<DownloadProgress>>,
    speed_tick: Duration,
) -> color_eyre::Result<()> {
    use std::io::{Read as _, Write as _};

    let mut writer = std::fs::File::create(part)
        .wrap_err_with(|| format!("create download part {}", part.display()))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut ema = None::<u64>;
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let bytes = buffer
            .get(..read)
            .ok_or_else(|| eyre!("opened reader returned invalid byte count"))?;
        writer.write_all(bytes)?;
        done = done.saturating_add(u64::try_from(read)?);
        let mut state = progress.lock();
        state.bytes_done = done;
        if let Some(total) = byte_len {
            state.bytes_total = total;
        }
        let elapsed = window_start.elapsed();
        if elapsed >= speed_tick {
            let elapsed_ms = u64::try_from(elapsed.as_millis())?.max(1);
            let instant = done.saturating_sub(window_bytes).saturating_mul(1000) / elapsed_ms;
            let smoothed = ema.map_or(instant, |old| {
                (old.saturating_mul(3) + instant.saturating_mul(2)) / 5
            });
            ema = Some(smoothed);
            state.speed_bps = smoothed;
            window_start = Instant::now();
            window_bytes = done;
        }
    }
    writer.flush()?;
    if let Some(expected) = byte_len
        && done < expected
    {
        return Err(eyre!("download truncated: {done} / {expected} bytes"));
    }
    Ok(())
}

/// 把一首已下完的 capture 文件后台收编进缓存(spawn_blocking,不阻塞 loop)。
/// 文件缺失 / 空(下载未完成)→ 不入缓存并删残件。
///
/// # Params:
///   - `player`: 播放核心(取 media_cache)
///   - `cap`: 该曲的 capture 上下文
pub(crate) fn spawn_harvest(player: &PlayerCore, cap: Capturing) {
    let cache = Arc::clone(player.media_cache());
    let player = player.clone();
    // 埋点用:song / quality / format 先留(cap 随后在 match 里借用)。format 未知时落
    // 显式 "unknown"(cache_harvests.format 为 NOT NULL,比空串更可辨)。
    let song_id = cap.song.id.clone();
    let quality = cap.quality.as_str().to_owned();
    let format = cap
        .format
        .as_ref()
        .map_or("unknown", mineral_model::AudioFormat::as_str)
        .to_owned();
    // async task(非 spawn_blocking):put_played 要 await DB 写穿透;入库内部的大拷贝由它自己
    // 再下沉到 spawn_blocking。metadata 是一次快速 stat,async 里直接调可接受。
    tokio::spawn(async move {
        let (outcome, bytes) = match std::fs::metadata(&cap.path) {
            Ok(m) if m.len() > 0 => {
                let bytes = i64::try_from(m.len()).ok();
                match cache
                    .put_played(&cap.song, cap.quality, cap.format.as_ref(), &cap.path)
                    .await
                {
                    Err(e) => {
                        mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "音频入缓存失败");
                        (mineral_stats::CacheHarvestOutcome::Discarded, bytes)
                    }
                    Ok(evicted) => {
                        // 埋点:本次入库触发的 LRU 驱逐(cache_evictions;系统域,无 actor)。
                        for ev in evicted {
                            player.inner.stats.event(mineral_stats::StatsEvent::System(
                                mineral_stats::SystemEvent::CacheEviction {
                                    cache_key: ev.key,
                                    bytes: i64::try_from(ev.bytes).unwrap_or(i64::MAX),
                                },
                            ));
                        }
                        if let Some(path) = cache.get(&cap.song.id, cap.quality) {
                            // 收割成功 = 该曲首次拥有完整本地副本:补算包络。若它仍在播,
                            // 算完即推,播放中段波形直接点亮;不在播则落库待下次直取。
                            player.ensure_envelope(cap.song.id.clone(), path.clone());
                            // 缓存文件落盘 → 异步打标(写引擎走副本 + rename,不伤在播 fd)。
                            player
                                .tagging()
                                .enqueue(cap.song.clone(), path, cap.quality);
                        }
                        (mineral_stats::CacheHarvestOutcome::Cached, bytes)
                    }
                }
            }
            _ => {
                mineral_log::debug!(target: "player", "capture 文件缺失/空,不入缓存");
                drop(std::fs::remove_file(&cap.path));
                (mineral_stats::CacheHarvestOutcome::Discarded, None)
            }
        };
        // 埋点:边播边收割结局(cache_harvests;系统域)。
        player.inner.stats.event(mineral_stats::StatsEvent::System(
            mineral_stats::SystemEvent::CacheHarvest {
                song: song_id,
                quality,
                format,
                outcome,
                bytes,
            },
        ));
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
    let Some(music_dir) = player.music_dir() else {
        player.notify().toast(
            mineral_protocol::ToastKind::Warn,
            "下载不可用(无音乐导出目录)".to_owned(),
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
    let hooks = player.hook_gate();
    let env = DownloadEnv {
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
            player.playback(),
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
                hooked,
            }) => {
                p.last_ok += 1;
                drop(p);
                player
                    .notify()
                    .download_completed(song, &path, quality, format.as_ref());
                player
                    .tagging()
                    .enqueue(song.clone(), path.clone(), quality);
                let path_str = path.display().to_string();
                record_download(
                    player,
                    song,
                    quality.as_str(),
                    format.as_ref().map(mineral_model::AudioFormat::as_str),
                    mineral_stats::DownloadOutcome::Downloaded,
                    hooked,
                    Some(path_str.as_str()),
                );
                p = player.progress_handle().lock();
            }
            Ok(DownloadOutcome::Skipped { cause }) => {
                p.last_skip += 1;
                drop(p);
                // 幂等跳过 hooked=none;脚本 veto 记 skip——两类跳过在库里分得开。
                let hooked = match cause {
                    super::download::SkipCause::AlreadyExists => mineral_stats::DownloadHook::None,
                    super::download::SkipCause::HookVeto => mineral_stats::DownloadHook::Skip,
                };
                record_download(
                    player,
                    song,
                    player.download_quality().as_str(),
                    None,
                    mineral_stats::DownloadOutcome::Skipped,
                    hooked,
                    None,
                );
            }
            Err(e) => {
                drop(p);
                mineral_log::warn!(target: "download", song_id = song.id.as_str(), error = mineral_log::chain(&e), "下载失败");
                record_download(
                    player,
                    song,
                    player.download_quality().as_str(),
                    None,
                    mineral_stats::DownloadOutcome::Failed,
                    mineral_stats::DownloadHook::None,
                    None,
                );
                player.progress_handle().lock().last_fail += 1;
            }
        }
    }
}

/// 记一次下载事件(system 触发;`hooked` 由 [`DownloadOutcome`] / [`SkipCause`] 带出)。
#[allow(clippy::too_many_arguments)] // downloads 一行的固有列,拆结构体反增噪
fn record_download(
    player: &PlayerCore,
    song: &Song,
    quality: &str,
    format: Option<&str>,
    outcome: mineral_stats::DownloadOutcome,
    hooked: mineral_stats::DownloadHook,
    path: Option<&str>,
) {
    player
        .inner
        .stats
        .event(mineral_stats::StatsEvent::Behavior {
            actor: mineral_stats::Actor::System,
            event: mineral_stats::BehaviorEvent::Download {
                song: song.id.clone(),
                quality: quality.to_owned(),
                format: format.map(str::to_owned),
                outcome,
                hooked,
                path: path.map(str::to_owned),
            },
        });
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
                .ok_or_else(|| "下载失败: 该来源无对应 channel".to_owned())?;
            channel
                .playlist_detail(id)
                .await
                .map(|playlist| {
                    playlist
                        .entries
                        .into_iter()
                        .map(|entry| entry.song)
                        .collect()
                })
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
    use mineral_playback::{PlaybackProvider, PlaybackRegistry};
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

    /// Registers one fixed-URL test playback provider.
    fn playback(channel: UrlChannel) -> color_eyre::Result<PlaybackRegistry> {
        let provider: Arc<dyn PlaybackProvider> = Arc::new(channel);
        PlaybackRegistry::new(vec![provider])
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
        let playback = playback(UrlChannel { url })?;
        let progress = Arc::new(Mutex::new(DownloadProgress::default()));
        let s = song();

        let outcome = download_song(
            &playback,
            &super::DownloadEnv {
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
        let playback = playback(UrlChannel {
            url: "http://127.0.0.1:9/dead.flac".parse()?,
        })?;
        let (runtime, gate) = script_gate(
            r#"
            mineral.hook("before_download", function(ctx)
                return { skip = "脚本拒绝" }
            end)
            "#,
        )?;
        let outcome = download_song(
            &playback,
            &super::DownloadEnv {
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
            matches!(outcome, DownloadOutcome::Skipped { .. }),
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
        let playback = playback(UrlChannel {
            url: "http://127.0.0.1:9/dead.flac".parse()?,
        })?;
        let (runtime, gate) = script_gate(&format!(
            r#"
            mineral.hook("before_download", function(ctx)
                return {{ url = "{live}", quality = "standard" }}
            end)
            "#
        ))?;
        let outcome = download_song(
            &playback,
            &super::DownloadEnv {
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
}
