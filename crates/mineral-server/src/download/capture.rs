//! Completed playback captures admitted to the audio cache.

use std::path::PathBuf;

use mineral_model::{AudioFormat, BitRate, Song};

use crate::player::PlayerCore;

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
