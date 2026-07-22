//! 查询测试族共享的时间锚点 + 播放事实 fixture(`open_temp` / `play` / `seed`)。

use std::ops::Range;

use crate::context::QueueContext;
use crate::play::PlayRecord;
use crate::report::ReportOptions;
use crate::store::StatsStore;
use crate::vocab::{Actor, FinishReason, PlayOrigin, PlaybackOrigin};
use mineral_model::{AudioFormat, BitRate};

use super::shared::song_id;

/// 2026-07-14 00:00:00 UTC(周二),种子时间锚点。
pub(super) const T0: i64 = 1_783_987_200_000;
pub(super) const HOUR: i64 = 3_600_000;
pub(super) const DAY: i64 = 86_400_000;

pub(super) async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
    let dir = tempfile::tempdir()?;
    let store = StatsStore::open(&dir.path().join("stats.db")).await?;
    Ok((dir, store))
}

/// 造一行播放事实(只填查询关心的列,其余取中性值)。
// 测试构造器:8 个入参各是独立的种子维度、无自然分组,豁免多参数 lint。
#[allow(clippy::too_many_arguments)]
pub(super) fn play(
    ns: &str,
    value: &str,
    started_at: i64,
    listen_ms: i64,
    finish: FinishReason,
    format: Option<AudioFormat>,
    quality: Option<BitRate>,
    session_id: i64,
) -> PlayRecord {
    PlayRecord {
        song_id: song_id(ns, value),
        started_at,
        ended_at: started_at + listen_ms,
        listen_ms,
        duration_ms_snapshot: None,
        finish_reason: finish,
        skip_at_ms: matches!(finish, FinishReason::Skip).then_some(listen_ms),
        play_mode: crate::PlayMode::Sequential,
        session_id,
        origin: PlayOrigin::Explicit,
        actor: Actor::User,
        context: QueueContext::Unknown,
        audio: crate::PlayAudioSnapshot {
            audio_format: format,
            bitrate_bps: None,
            quality,
            bit_depth: None,
            substituted: false,
        },
        playback_origin: PlaybackOrigin::Remote,
    }
}

/// 确定性种子:A×2(netease,flac 无损,day1 14 时)、B×1(netease,mp3,skip,
/// 5s 不足有效阈值,day1 15 时)、C×1(bilibili,无格式,day2 09 时)。
pub(super) async fn seed(store: &StatsStore) -> color_eyre::Result<i64> {
    let sid = store
        .open_session(T0)
        .await?
        .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
    let f = |fmt: &str| Some(AudioFormat::from(fmt.to_owned()));
    store
        .record_play(&play(
            "netease",
            "1",
            T0 + 14 * HOUR,
            60_000,
            FinishReason::Eof,
            f("flac"),
            Some(BitRate::Lossless),
            sid,
        ))
        .await?;
    store
        .record_play(&play(
            "netease",
            "1",
            T0 + 14 * HOUR + 60_000,
            70_000,
            FinishReason::Eof,
            f("flac"),
            Some(BitRate::Lossless),
            sid,
        ))
        .await?;
    store
        .record_play(&play(
            "netease",
            "2",
            T0 + 15 * HOUR,
            5_000,
            FinishReason::Skip,
            f("mp3"),
            Some(BitRate::Exhigh),
            sid,
        ))
        .await?;
    store
        .record_play(&play(
            "bilibili",
            "3",
            T0 + DAY + 9 * HOUR,
            120_000,
            FinishReason::Eof,
            None,
            None,
            sid,
        ))
        .await?;
    Ok(sid)
}

/// 覆盖全部种子的时间窗。
pub(super) fn full_range() -> Range<i64> {
    T0..(T0 + 2 * DAY)
}

pub(super) fn options(min_listen_ms: i64) -> ReportOptions {
    ReportOptions::builder()
        .min_listen_ms(min_listen_ms)
        .top_limit(10)
        .build()
}
