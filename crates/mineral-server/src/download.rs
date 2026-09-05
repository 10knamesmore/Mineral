//! Provider-backed download/export draining decoder-ready opened media to permanent storage.
//!
//! 这是可复用 transfer 单元；session lifecycle、并发 admission 与 Stop 由 [`DownloadManager`] 负责。
//! 导出落 `<music_dir>/<source>/<quality>/<album>/<title>.ext`；目标路径存在即跳过，不覆盖也不分配副本。
//! 导出永久保留，不受缓存 LRU 驱逐；
//! 播放解析(见 [`crate::resolve`])直接探测该目录命中,不复制进缓存。provider resolve/open
//! 产生 decoder-ready encoded media，export 顺序写入 preparation 后的 bytes。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, eyre};
use mineral_model::{AudioFormat, BitRate, Song};
use mineral_playback::{OpenOptions, PlaybackRegistry, PlaybackRequest};
use mineral_protocol::DownloadId;
use tokio_util::sync::CancellationToken;

use crate::media_cache::library_relpath;
use crate::player::PlayerCore;

mod manager;

pub(crate) use manager::{DownloadManager, DownloadRuntime};

/// Callback shared by the async opener and blocking writer.
type TransferReporter = Arc<dyn Fn(TransferUpdate) + Send + Sync>;

/// Identity and cancellation state of one admitted attempt.
pub(crate) struct DownloadAttempt<'a> {
    /// Session-local Song download identity.
    pub(crate) id: &'a DownloadId,

    /// Cooperative cancellation handle.
    pub(crate) cancellation: &'a CancellationToken,
}

/// Transfer progress reported back to the lifecycle owner.
#[derive(Clone, Copy)]
pub(crate) enum TransferUpdate {
    /// Opened media is being drained.
    Downloading {
        /// Effective quality after hook/provider resolution.
        quality: BitRate,

        /// Bytes written to the partial.
        bytes_done: u64,

        /// Provider-declared total bytes, when available.
        bytes_total: Option<u64>,

        /// Smoothed bytes per second.
        speed_bps: u64,
    },

    /// The complete partial is about to be committed.
    Finalizing,
}

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

        /// Quality identity used by the skipped export decision.
        quality: BitRate,
    },
}

/// 一次下载跳过的成因(埋点 hooked 列据此分流)。
#[derive(Clone, Copy, Debug)]
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
/// 该歌该音质的派生标题路径已存在则跳过，不调用 provider。
///
/// 导出目录本身即播放解析的命中源(见 [`crate::resolve`]),不复制进缓存——复制只会
/// 徒增双份存储、并让播放走 LRU 副本而非永久文件。
///
/// # Params:
///   - `playback`: Playback provider registry.
///   - `song`: 要下载的歌
///   - `quality`: 下载音质
///   - `env`: 下载环境(导出根目录 + 脚本拦截门)
///   - `attempt`: Unique download identity and cancellation handle.
///   - `reporter`: Lifecycle progress callback.
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
    attempt: DownloadAttempt<'_>,
    reporter: TransferReporter,
    speed_tick: Duration,
) -> color_eyre::Result<DownloadOutcome> {
    let DownloadEnv { music_dir, hooks } = *env;
    if crate::resolve::probe_export(music_dir, song, quality).is_some() {
        mineral_log::debug!(target: "download", song_id = song.id.as_str(), "已下载,跳过");
        return Ok(DownloadOutcome::Skipped {
            cause: SkipCause::AlreadyExists,
            quality,
        });
    }

    let provider = playback
        .get(song.source())
        .ok_or_else(|| eyre!("no playback provider for {:?}", song.source()))?;
    let mut prepared = provider
        .resolve(
            PlaybackRequest::new(song.id.clone(), quality),
            attempt.cancellation.child_token(),
        )
        .await?;
    ensure_not_cancelled(attempt.cancellation)?;

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
                quality,
            });
        }
    }
    let opened = prepared
        .open(OpenOptions::new(
            attempt.cancellation.child_token(),
            /*prefetch_bytes*/ 0,
        ))
        .await?;
    ensure_not_cancelled(attempt.cancellation)?;
    let quality = opened.info().quality;
    let format = opened.info().format.clone();
    let byte_len = opened.byte_len();
    let (subdir, file_name) = library_relpath(song, quality, format.as_ref());
    let export = music_dir.join(&subdir).join(&file_name);
    if let Some(parent) = export.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .wrap_err_with(|| format!("创建导出目录失败 {}", parent.display()))?;
    }
    let part = owned_partial_path(&export, attempt.id);
    let reader = opened.into_reader();
    reporter(TransferUpdate::Downloading {
        quality,
        bytes_done: 0,
        bytes_total: byte_len,
        speed_bps: 0,
    });
    let progress_reporter = Arc::clone(&reporter);
    let cancellation = attempt.cancellation.clone();
    let part_for_write = part.clone();
    let write_result = tokio::task::spawn_blocking(move || {
        drain_opened(
            reader,
            &part_for_write,
            quality,
            byte_len,
            &cancellation,
            &progress_reporter,
            speed_tick,
        )
    })
    .await
    .map_err(|error| eyre!("download writer task: {error}"))?;
    if let Err(error) = write_result {
        remove_owned_partial(&part).await;
        return Err(error);
    }
    if let Err(error) = ensure_not_cancelled(attempt.cancellation) {
        remove_owned_partial(&part).await;
        return Err(error);
    }
    reporter(TransferUpdate::Finalizing);
    if let Err(error) = ensure_not_cancelled(attempt.cancellation) {
        remove_owned_partial(&part).await;
        return Err(error);
    }
    match tokio::fs::hard_link(&part, &export).await {
        Ok(()) => {
            remove_owned_partial(&part).await;
            mineral_log::info!(target: "download", song_id = song.id.as_str(), path = %export.display(), "下载完成");
            Ok(DownloadOutcome::Downloaded {
                path: export,
                quality,
                format,
                hooked,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            remove_owned_partial(&part).await;
            Ok(DownloadOutcome::Skipped {
                cause: SkipCause::AlreadyExists,
                quality,
            })
        }
        Err(error) => {
            remove_owned_partial(&part).await;
            Err(error).wrap_err_with(|| format!("commit download export {}", export.display()))
        }
    }
}

/// 已由 producer 校验完整、等待收编进音频缓存的播放媒体。
pub(crate) struct CaptureHarvest {
    /// 在播的歌(组库路径取 source / album / title)。
    pub(crate) song: Song,

    /// 入库音质(与播放请求一致,决定 index 键 / 目录)。
    pub(crate) quality: BitRate,

    /// 实际音频格式(决定扩展名;未知按音质兜底)。
    pub(crate) format: Option<AudioFormat>,

    /// 已完整落盘的唯一临时路径。
    pub(crate) path: PathBuf,
}

/// Drains one synchronous opened-media reader into a permanent part file.
fn drain_opened(
    mut reader: Box<dyn mineral_playback::MediaReader>,
    part: &Path,
    quality: BitRate,
    byte_len: Option<u64>,
    cancellation: &CancellationToken,
    reporter: &TransferReporter,
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
        ensure_not_cancelled(cancellation)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let bytes = buffer
            .get(..read)
            .ok_or_else(|| eyre!("opened reader returned invalid byte count"))?;
        writer.write_all(bytes)?;
        done = done.saturating_add(u64::try_from(read)?);
        let elapsed = window_start.elapsed();
        if elapsed >= speed_tick {
            let elapsed_ms = u64::try_from(elapsed.as_millis())?.max(1);
            let instant = done.saturating_sub(window_bytes).saturating_mul(1000) / elapsed_ms;
            let smoothed = ema.map_or(instant, |old| {
                (old.saturating_mul(3) + instant.saturating_mul(2)) / 5
            });
            ema = Some(smoothed);
            reporter(TransferUpdate::Downloading {
                quality,
                bytes_done: done,
                bytes_total: byte_len,
                speed_bps: smoothed,
            });
            window_start = Instant::now();
            window_bytes = done;
        }
    }
    writer.flush()?;
    reporter(TransferUpdate::Downloading {
        quality,
        bytes_done: done,
        bytes_total: byte_len,
        speed_bps: ema.unwrap_or_default(),
    });
    if let Some(expected) = byte_len
        && done < expected
    {
        return Err(eyre!("download truncated: {done} / {expected} bytes"));
    }
    Ok(())
}

/// Fails an attempt after cooperative cancellation was requested.
fn ensure_not_cancelled(cancellation: &CancellationToken) -> color_eyre::Result<()> {
    if cancellation.is_cancelled() {
        Err(eyre!("download stopped"))
    } else {
        Ok(())
    }
}

/// Builds the partial path owned by this download's sole execution.
fn owned_partial_path(export: &Path, id: &DownloadId) -> PathBuf {
    let extension = export
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("media");
    export.with_extension(format!("{extension}.mineral-{}.part-dl", id.as_str()))
}

/// Removes one known owned partial, ignoring an already-absent file.
async fn remove_owned_partial(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            mineral_log::warn!(target: "download", path = %path.display(), error = mineral_log::chain(&error), "failed to remove owned partial");
        }
    }
}

/// Removes crash leftovers before the manager accepts any new work.
pub(crate) fn cleanup_orphan_partials(root: &Path) {
    match cleanup_owned_tree(root) {
        Ok(removed) if removed > 0 => {
            mineral_log::info!(target: "download", root = %root.display(), removed, "orphan download partials removed");
        }
        Ok(_) => {}
        Err(error) => {
            mineral_log::warn!(target: "download", root = %root.display(), error = mineral_log::chain(&error), "orphan partial cleanup failed");
        }
    }
}

/// Recursively removes only files matching Mineral's owned partial suffix contract.
fn cleanup_owned_tree(root: &Path) -> color_eyre::Result<usize> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error).wrap_err_with(|| format!("read {}", root.display())),
    };
    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.wrap_err_with(|| format!("read entry under {}", root.display()))?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            removed = removed.saturating_add(cleanup_owned_tree(&path)?);
        } else if file_type.is_file() && is_owned_partial(&path) {
            std::fs::remove_file(&path)
                .wrap_err_with(|| format!("remove orphan partial {}", path.display()))?;
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

/// Whether a file name proves it belongs to the manager's unique partial contract.
fn is_owned_partial(path: &Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.contains(".mineral-") && name.ends_with(".part-dl"))
}

/// 把一首 producer 已校验完整的 capture 文件收编进缓存。
/// 文件在等待期间消失或变空时不入缓存，并清理残件。
///
/// # Params:
///   - `player`: 播放核心(取 media_cache)
///   - `cap`: 已完成 capture 的歌曲、媒体事实与临时路径
pub(crate) async fn harvest_capture(player: &PlayerCore, cap: CaptureHarvest) {
    let cache = player.media_cache();
    // 埋点用:song / quality / format 先留(cap 随后在 match 里借用)。format 未知时落
    // 显式 "unknown"(cache_harvests.format 为 NOT NULL,比空串更可辨)。
    let song_id = cap.song.id.clone();
    let quality = cap.quality.as_str().to_owned();
    let format = cap
        .format
        .as_ref()
        .map_or("unknown", mineral_model::AudioFormat::as_str)
        .to_owned();
    let (outcome, bytes) = match std::fs::metadata(&cap.path) {
        Ok(metadata) if metadata.len() > 0 => {
            let bytes = i64::try_from(metadata.len()).ok();
            match cache
                .put_played(&cap.song, cap.quality, cap.format.as_ref(), &cap.path)
                .await
            {
                Err(error) => {
                    mineral_log::warn!(target: "player", error = mineral_log::chain(&error), "音频入缓存失败");
                    (mineral_stats::CacheHarvestOutcome::Discarded, bytes)
                }
                Ok(evicted) => {
                    for eviction in evicted {
                        player.inner.stats.event(mineral_stats::StatsEvent::System(
                            mineral_stats::SystemEvent::CacheEviction {
                                cache_key: eviction.key,
                                bytes: i64::try_from(eviction.bytes).unwrap_or(i64::MAX),
                            },
                        ));
                    }
                    if let Some(path) = cache.get(&cap.song.id, cap.quality) {
                        mineral_log::info!(
                            target: "player",
                            song_id = %cap.song.id.qualified(),
                            path = %path.display(),
                            "playback capture cached"
                        );
                        // 当前曲会在计算完成时收到包络；已切走或仍在预排时先落库供之后重放。
                        player.ensure_envelope(cap.song.id.clone(), path.clone());
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
    player.inner.stats.event(mineral_stats::StatsEvent::System(
        mineral_stats::SystemEvent::CacheHarvest {
            song: song_id,
            quality,
            format,
            outcome,
            bytes,
        },
    ));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_model::{AlbumId, AlbumRef, BitRate, Song, SongId, SourceKind};
    use mineral_persist::ServerStore;
    use mineral_playback::{PlaybackProvider, PlaybackRegistry};
    use mineral_protocol::DownloadId;
    use mineral_test::mock::{UrlChannel, serve_once};
    use tokio_util::sync::CancellationToken;

    use std::time::Duration;

    use super::{DownloadAttempt, DownloadEnv, DownloadOutcome, download_song};
    use crate::media_cache::MediaCache;

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

    /// Runs one transfer with an isolated identity and no-op progress reporter.
    async fn download_for_test(
        playback: &PlaybackRegistry,
        env: &DownloadEnv<'_>,
        song: &Song,
        quality: BitRate,
    ) -> color_eyre::Result<DownloadOutcome> {
        let id = DownloadId::new("test-download".to_owned());
        let cancellation = CancellationToken::new();
        download_song(
            playback,
            env,
            song,
            quality,
            DownloadAttempt {
                id: &id,
                cancellation: &cancellation,
            },
            Arc::new(|_update| {}),
            /*speed_tick*/ Duration::from_millis(150),
        )
        .await
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
        let s = song();

        let outcome = download_for_test(
            &playback,
            &DownloadEnv {
                music_dir: &music_dir,
                hooks: &crate::hook_bridge::HookGate::disabled(),
            },
            &s,
            BitRate::Lossless,
        )
        .await?;
        assert!(
            matches!(outcome, DownloadOutcome::Downloaded { .. }),
            "应真正下载"
        );
        assert!(
            crate::resolve::probe_export(&music_dir, &s, BitRate::Lossless).is_some(),
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
        let outcome = download_for_test(
            &playback,
            &DownloadEnv {
                music_dir: &music_dir,
                hooks: &gate,
            },
            &song(),
            BitRate::Lossless,
        )
        .await?;
        assert!(
            matches!(outcome, DownloadOutcome::Skipped { .. }),
            "hook 跳过应记 Skipped"
        );
        assert!(
            crate::resolve::probe_export(&music_dir, &song(), BitRate::Lossless).is_none(),
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
        let outcome = download_for_test(
            &playback,
            &DownloadEnv {
                music_dir: &music_dir,
                hooks: &gate,
            },
            &song(),
            BitRate::Lossless,
        )
        .await?;
        assert!(
            matches!(outcome, DownloadOutcome::Downloaded { .. }),
            "改写到活地址应下载成功"
        );
        assert!(
            crate::resolve::probe_export(&music_dir, &song(), BitRate::Standard).is_some(),
            "导出路径应按改写后的音质(standard)标注"
        );
        drop(runtime);
        Ok(())
    }
}
