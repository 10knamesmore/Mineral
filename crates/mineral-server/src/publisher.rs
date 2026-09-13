//! 领域状态发布器:daemon 侧一次构造、多订阅者共享的版本化快照源。
//!
//! 每个领域一条采样 / 通知链路,产物进 `watch`(只保留最新值)或 `broadcast`
//! (增量包)。订阅会话各自转发,不按连接重建整份领域快照;没有订阅者时采样仍然
//! 廉价(单次读 + 比较),但不会构造 per-connection 数据。

use std::sync::Arc;
use std::time::Duration;

use mineral_audio::{AudioHandle, AudioSnapshot};
use mineral_protocol::{
    DownloadDetailDelta, DownloadDetailUpdate, DownloadSummary, Event, SongDownloadView,
};
use mineral_task::Snapshot;
use rustc_hash::FxHashMap;
use tokio::sync::{broadcast, watch};

use crate::pcm_relay::PcmRelay;
use crate::player::PlayerCore;

/// 播放锚点兜底采样节拍:快照写入信号负责即时发布,此拍只兜底漏发的唤醒,
/// 并充当位置重锚的检查节拍。
const PLAYBACK_SAMPLE_MS: u64 = 100;

/// 播放中重锚周期(位置由 client 本地推进,这里只做周期校准)。
const PLAYBACK_REANCHOR_MS: u64 = 100;

/// 任务摘要采样节奏。
const TASKS_SAMPLE_MS: u64 = 250;

/// 下载摘要采样节奏。
const DOWNLOADS_SUMMARY_SAMPLE_MS: u64 = 250;

/// 下载明细变更去抖(合并连续进度)。
const DOWNLOADS_DETAIL_DEBOUNCE_MS: u64 = 100;

/// 会话可订阅的全部领域发布源。
#[derive(Clone)]
pub(crate) struct DomainPublishers {
    /// 播放锚点最新值。
    playback: watch::Receiver<Arc<AudioSnapshot>>,

    /// 任务摘要最新值。
    tasks: watch::Receiver<Arc<Snapshot>>,

    /// 下载摘要最新值。
    downloads_summary: watch::Receiver<Arc<DownloadSummary>>,

    /// 下载明细完整快照最新值(新订阅者首帧)。
    downloads_detail: watch::Receiver<Option<Arc<Vec<SongDownloadView>>>>,

    /// 下载明细增量包。
    downloads_delta: broadcast::Sender<Arc<DownloadDetailDelta>>,

    /// PCM 中继。
    pcm: PcmRelay,

    /// 事件 hub(Toast / Lifecycle / Config / WindowTitle / Task / Bus / Property)。
    events: broadcast::Sender<Event>,
}

impl DomainPublishers {
    /// 订阅播放锚点。
    pub(crate) fn playback(&self) -> watch::Receiver<Arc<AudioSnapshot>> {
        self.playback.clone()
    }

    /// 订阅任务摘要。
    pub(crate) fn tasks(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.tasks.clone()
    }

    /// 订阅下载摘要。
    pub(crate) fn downloads_summary(&self) -> watch::Receiver<Arc<DownloadSummary>> {
        self.downloads_summary.clone()
    }

    /// 下载明细完整快照(新订阅首帧)。
    pub(crate) fn downloads_detail(&self) -> watch::Receiver<Option<Arc<Vec<SongDownloadView>>>> {
        self.downloads_detail.clone()
    }

    /// 订阅下载明细增量。
    pub(crate) fn downloads_delta(&self) -> broadcast::Receiver<Arc<DownloadDetailDelta>> {
        self.downloads_delta.subscribe()
    }

    /// PCM 中继。
    pub(crate) fn pcm(&self) -> &PcmRelay {
        &self.pcm
    }

    /// 事件 hub 订阅端。
    pub(crate) fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}

/// 起全部领域发布链路。
///
/// # Params:
///   - `player`: 业务核心(读播放状态 / 任务 / 下载)
///   - `events`: 事件 hub(已有生产者)
///   - `pcm`: PCM 中继
pub(crate) fn spawn(
    player: &PlayerCore,
    events: broadcast::Sender<Event>,
    pcm: PcmRelay,
) -> DomainPublishers {
    let playback = spawn_playback_publisher(player.audio().clone());
    let tasks = spawn_tasks_publisher(player.clone());
    let (downloads_summary, downloads_detail, downloads_delta) = spawn_downloads_publishers(player);
    DomainPublishers {
        playback,
        tasks,
        downloads_summary,
        downloads_detail,
        downloads_delta,
        pcm,
        events,
    }
}

/// 采样播放锚点:状态签名变化时发布,播放中按重锚周期校准位置。
///
/// 快照写入会立即唤醒(见 [`AudioHandle::snapshot_changes`]),音量 / 起停这类
/// 由写入点驱动的变化因此不必等采样拍。
fn spawn_playback_publisher(audio: AudioHandle) -> watch::Receiver<Arc<AudioSnapshot>> {
    let (tx, rx) = watch::channel(Arc::new(audio.snapshot()));
    let changes = audio.snapshot_changes();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(PLAYBACK_SAMPLE_MS));
        let mut last_signature: Option<PlaybackSignature> = None;
        let mut last_sent = std::time::Instant::now();
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                () = changes.notified() => {}
            }
            let snapshot = audio.snapshot();
            let signature = PlaybackSignature::of(&snapshot);
            let changed = last_signature.as_ref() != Some(&signature);
            let reanchor = snapshot.playing
                && last_sent.elapsed() >= Duration::from_millis(PLAYBACK_REANCHOR_MS);
            if changed || reanchor {
                last_signature = Some(signature);
                last_sent = std::time::Instant::now();
                let _ = tx.send(Arc::new(snapshot));
            }
        }
    });
    rx
}

/// 播放状态签名:位置之外的一切(位置由 client 本地推进,不进签名)。
#[derive(PartialEq, Eq)]
struct PlaybackSignature {
    /// 是否在出声。
    playing: bool,

    /// 时长。
    duration_ms: Option<u64>,

    /// 音量。
    volume_pct: u8,

    /// 曲终 latch。
    track_finished_seq: u64,

    /// 后端形态。
    backend: mineral_audio::AudioBackend,

    /// 轨道令牌。
    current_track_token: u64,

    /// 预排时长。
    next_duration_ms: Option<u64>,

    /// 预排就绪。
    next_ready: bool,

    /// 采样率。
    sample_rate_hz: u32,
}

impl PlaybackSignature {
    /// 从快照取签名。
    ///
    /// # Params:
    ///   - `snapshot`: 音频快照
    fn of(snapshot: &AudioSnapshot) -> Self {
        Self {
            playing: snapshot.playing,
            duration_ms: snapshot.duration_ms,
            volume_pct: snapshot.volume_pct,
            track_finished_seq: snapshot.track_finished_seq,
            backend: snapshot.backend,
            current_track_token: snapshot.current_track_token,
            next_duration_ms: snapshot.next_duration_ms,
            next_ready: snapshot.next_ready,
            sample_rate_hz: snapshot.sample_rate_hz,
        }
    }
}

/// 任务摘要发布:变化才发(初值即当前摘要,新订阅者立刻拿到)。
fn spawn_tasks_publisher(player: PlayerCore) -> watch::Receiver<Arc<Snapshot>> {
    let initial = player.task_snapshot();
    let (tx, rx) = watch::channel(Arc::new(initial.clone()));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(TASKS_SAMPLE_MS));
        let mut last = Some(initial);
        loop {
            tick.tick().await;
            let snapshot = player.task_snapshot();
            let changed = last.as_ref().is_none_or(|previous| {
                previous.running != snapshot.running || previous.by_kind != snapshot.by_kind
            });
            if changed {
                last = Some(snapshot.clone());
                let _ = tx.send(Arc::new(snapshot));
            }
        }
    });
    rx
}

/// 下载订阅流集合:摘要 watch + 明细快照 watch + 明细增量广播。
type DownloadStreams = (
    watch::Receiver<Arc<DownloadSummary>>,
    watch::Receiver<Option<Arc<Vec<SongDownloadView>>>>,
    broadcast::Sender<Arc<DownloadDetailDelta>>,
);

/// 下载摘要 / 明细发布:摘要变化才发;明细按变更通知去抖后产增量包。
fn spawn_downloads_publishers(player: &PlayerCore) -> DownloadStreams {
    let (summary_tx, summary_rx) = watch::channel(Arc::new(player.download_summary()));
    let (detail_tx, detail_rx) = watch::channel(None);
    let (delta_tx, _delta_rx) =
        broadcast::channel::<Arc<DownloadDetailDelta>>(/*capacity*/ 64);

    {
        let player = player.clone();
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(Duration::from_millis(DOWNLOADS_SUMMARY_SAMPLE_MS));
            let mut last = DownloadSummary::default();
            loop {
                tick.tick().await;
                let summary = player.download_summary();
                if summary != last {
                    last = summary.clone();
                    let _ = summary_tx.send(Arc::new(summary));
                }
            }
        });
    }
    {
        let player = player.clone();
        let delta_tx = delta_tx.clone();
        tokio::spawn(async move {
            let mut changes = player.download_changes();
            let mut published: Vec<SongDownloadView> = Vec::new();
            let mut initialized = false;
            loop {
                if changes.changed().await.is_err() {
                    return;
                }
                // 去抖:连续进度合并成一次发布。
                tokio::time::sleep(Duration::from_millis(DOWNLOADS_DETAIL_DEBOUNCE_MS)).await;
                let rows = player.download_snapshot();
                let delta = diff_downloads(&published, &rows, initialized);
                initialized = true;
                published = rows.clone();
                if delta.order.is_some() || !delta.changes.is_empty() {
                    let _ = detail_tx.send(Some(Arc::new(rows)));
                    let _ = delta_tx.send(Arc::new(delta));
                }
            }
        });
    }
    (summary_rx, detail_rx, delta_tx)
}

/// 计算下载明细增量(静态字段变化走 Upsert,连续进度走 Progress,缺行走 Remove)。
///
/// # Params:
///   - `previous`: 上次发布的完整列表
///   - `current`: 本次权威列表
///   - `initialized`: 是否已有基线(首帧不发增量,由完整快照承担)
fn diff_downloads(
    previous: &[SongDownloadView],
    current: &[SongDownloadView],
    initialized: bool,
) -> DownloadDetailDelta {
    let current_order = current.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    let previous_order = previous
        .iter()
        .map(|row| row.id.clone())
        .collect::<Vec<_>>();
    let order = (initialized && previous_order != current_order).then(|| current_order.clone());
    if !initialized {
        return DownloadDetailDelta {
            order: Some(current_order),
            changes: Vec::new(),
        };
    }
    let previous_by_id = previous
        .iter()
        .map(|row| (row.id.clone(), row))
        .collect::<FxHashMap<_, _>>();
    let current_ids = current_order
        .iter()
        .cloned()
        .collect::<rustc_hash::FxHashSet<_>>();
    let mut changes = Vec::new();
    for row in current {
        match previous_by_id.get(&row.id) {
            None => changes.push(DownloadDetailUpdate::Upsert(Box::new(row.clone()))),
            Some(old) => {
                let static_changed = old.song.id != row.song.id
                    || old.origin != row.origin
                    || old.quality != row.quality;
                if static_changed {
                    changes.push(DownloadDetailUpdate::Upsert(Box::new(row.clone())));
                    continue;
                }
                let dynamic_changed = old.status != row.status
                    || old.bytes_done != row.bytes_done
                    || old.bytes_total != row.bytes_total
                    || old.speed_bps != row.speed_bps
                    || old.failure != row.failure;
                if dynamic_changed {
                    changes.push(DownloadDetailUpdate::Progress {
                        id: row.id.clone(),
                        status: row.status,
                        bytes_done: row.bytes_done,
                        bytes_total: row.bytes_total,
                        speed_bps: row.speed_bps,
                        failure: row.failure.clone(),
                    });
                }
            }
        }
    }
    for old in previous {
        if !current_ids.contains(&old.id) {
            changes.push(DownloadDetailUpdate::Remove(old.id.clone()));
        }
    }
    DownloadDetailDelta { order, changes }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mineral_audio::{AudioHandle, AudioMode};
    use mineral_model::{BitRate, Song};
    use mineral_protocol::{DownloadId, DownloadOrigin, DownloadStatus, SongDownloadView};
    use mineral_test::song;
    use tokio::task::yield_now;
    use tokio::time::timeout;

    use super::{diff_downloads, spawn_playback_publisher};

    /// 音量变化立即推送,不等兜底采样拍。
    ///
    /// 上限 50ms 小于采样节拍 100ms:少了快照写入信号必然超时。
    #[tokio::test]
    async fn volume_change_publishes_without_waiting_for_sample_tick() -> color_eyre::Result<()> {
        let cfg = crate::config::ServerConfig::from_config(&mineral_config::Config::defaults()?);
        let (audio, _tap) = AudioHandle::spawn(AudioMode::ForceNull, cfg.engine().clone())?;
        let mut rx = spawn_playback_publisher(audio.clone());
        // 先让发布器把首帧发掉,后面只认音量那一次推送。
        yield_now().await;
        rx.borrow_and_update();

        audio.set_volume(42);

        timeout(Duration::from_millis(50), rx.changed())
            .await
            .map_err(|_elapsed| color_eyre::eyre::eyre!("音量变化未即时推送"))??;
        assert_eq!(rx.borrow().volume_pct, 42);
        Ok(())
    }

    /// 造一行下载明细。
    fn row(id: &str, status: DownloadStatus, bytes: u64) -> SongDownloadView {
        SongDownloadView {
            id: DownloadId::new(id.to_owned()),
            song: Box::new(Song::clone(&song(id))),
            origin: DownloadOrigin::Direct,
            status,
            quality: BitRate::Exhigh,
            bytes_done: bytes,
            bytes_total: Some(1_000),
            speed_bps: 10,
            failure: None,
        }
    }

    /// 首帧只给顺序;连续进度只发轻字段;新增走 Upsert;缺行走 Remove。
    #[test]
    fn diff_separates_static_and_progress() {
        let first = vec![row("a", DownloadStatus::Downloading, 10)];
        let initial = diff_downloads(&[], &first, /*initialized*/ false);
        assert!(initial.order.is_some());
        assert!(initial.changes.is_empty(), "首帧由完整快照承担行内容");

        let mut progressed = first.clone();
        if let Some(item) = progressed.first_mut() {
            item.bytes_done = 20;
        }
        let delta = diff_downloads(&first, &progressed, /*initialized*/ true);
        assert!(delta.order.is_none(), "顺序未变不该重发");
        assert!(
            matches!(
                delta.changes.as_slice(),
                [mineral_protocol::DownloadDetailUpdate::Progress { bytes_done: 20, .. }]
            ),
            "进度增量应只带轻字段: {:?}",
            delta.changes
        );

        let mut with_new = progressed.clone();
        with_new.insert(0, row("b", DownloadStatus::Queued, 0));
        let delta = diff_downloads(&progressed, &with_new, /*initialized*/ true);
        assert!(delta.order.is_some(), "顺序变化应显式下发");
        assert!(
            delta
                .changes
                .iter()
                .any(|change| matches!(change, mineral_protocol::DownloadDetailUpdate::Upsert(row) if row.id.as_str() == "b")),
            "新增行应走 Upsert: {:?}",
            delta.changes
        );

        let delta = diff_downloads(&with_new, &progressed, /*initialized*/ true);
        assert!(
            delta
                .changes
                .iter()
                .any(|change| matches!(change, mineral_protocol::DownloadDetailUpdate::Remove(id) if id.as_str() == "b")),
            "缺行应走 Remove: {:?}",
            delta.changes
        );
    }
}
