//! Download identity, Stop settlement, and history ordering regressions.

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{OptionExt, eyre};
use mineral_model::BitRate;
use mineral_persist::ServerStore;
use mineral_playback::PlaybackRegistry;
use mineral_protocol::{DownloadId, DownloadOrigin, DownloadStatus, SongDownloadView};
use mineral_test::mock::{UrlChannel, serve_once};
use mineral_test::song;
use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::{DownloadManager, DownloadRuntime, Lane, ManagerInner, ManagerState};
use crate::download::{
    DownloadAttempt, DownloadEnv, DownloadOutcome, TransferUpdate, download_song,
};

/// Builds a manager whose admission is stepped by the test instead of a background scheduler.
async fn manager(
    max_concurrent: usize,
    playback: PlaybackRegistry,
) -> color_eyre::Result<(DownloadManager, tempfile::TempDir)> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("state.db")).await?;
    let (events, _) = tokio::sync::broadcast::channel(/*capacity*/ 16);
    let runtime = DownloadRuntime {
        music_dir: Some(dir.path().join("music")),
        channels: Vec::new(),
        playback,
        hooks: crate::hook_bridge::HookGate::disabled(),
        tagging: crate::tagging::TaggingQueue::spawn(
            /*enabled*/ false,
            &[],
            /*http*/ None,
            &persist,
            /*workers*/ 1,
        ),
        notify: crate::notify::Notifier::new(events, /*script*/ None),
        stats: crate::StatsRecorder::disabled(),
        speed_tick: Duration::from_millis(/*millis*/ 150),
    };
    Ok((
        DownloadManager {
            inner: Arc::new(ManagerInner {
                runtime,
                state: Mutex::new(ManagerState::new(BitRate::Lossless, max_concurrent)),
                wake: Notify::new(),
                quiesced: Notify::new(),
                shutdown: CancellationToken::new(),
            }),
        },
        dir,
    ))
}

/// Reads the observable row for one download identity.
fn row(manager: &DownloadManager, id: &DownloadId) -> color_eyre::Result<SongDownloadView> {
    manager
        .snapshot()
        .into_iter()
        .find(|row| &row.id == id)
        .ok_or_eyre("download row missing")
}

/// Stop keeps capacity occupied until settlement; resubmission is isolated from old callbacks.
#[tokio::test]
async fn stopped_download_settles_before_resubmission() -> color_eyre::Result<()> {
    let (manager, _dir) = manager(/*max_concurrent*/ 1, PlaybackRegistry::empty()).await?;
    let song = song("same-song");
    manager.admit_song(&song, DownloadOrigin::Direct, Lane::Direct);
    let old = manager
        .take_next()
        .ok_or_eyre("first download not admitted")?;
    manager.stop(&old.id)?;
    manager.stop(&old.id)?;
    assert!(old.cancellation.is_cancelled());
    assert_eq!(manager.summary().active, 1, "停止中仍占用执行槽位");
    manager.report(&old.id, TransferUpdate::Finalizing);
    assert_eq!(row(&manager, &old.id)?.status, DownloadStatus::Stopping);
    manager.admit_song(&song, DownloadOrigin::Direct, Lane::Direct);
    assert_eq!(manager.snapshot().len(), 1, "旧下载收尾前仍去重");
    assert!(manager.take_next().is_none());

    manager.finish_attempt(&old, Err(eyre!("writer stopped")));
    assert_eq!(row(&manager, &old.id)?.status, DownloadStatus::Stopped);
    assert_eq!(manager.summary().active, 0);
    manager.admit_song(&song, DownloadOrigin::Direct, Lane::Direct);
    let new = manager
        .take_next()
        .ok_or_eyre("resubmission not admitted")?;
    assert_ne!(old.id, new.id, "重新提交必须创建独立身份");
    manager.report(&old.id, TransferUpdate::Finalizing);
    manager.finish_attempt(&old, Err(eyre!("late completion")));
    assert_eq!(row(&manager, &new.id)?.status, DownloadStatus::Resolving);
    assert_eq!(manager.summary().active, 1);
    manager.stop(&new.id)?;
    manager.finish_attempt(&new, Err(eyre!("writer stopped")));
    assert!(
        manager
            .stop(&DownloadId::new("unknown".to_owned()))
            .is_err()
    );
    Ok(())
}

/// Active and queued rows precede the latest 100 completions, regardless of admission order.
#[tokio::test]
async fn snapshot_orders_active_queued_and_recent_history() -> color_eyre::Result<()> {
    let (manager, _dir) = manager(/*max_concurrent*/ 2, PlaybackRegistry::empty()).await?;
    let queued_song = song("queued-first");
    let origin = DownloadOrigin::Playlist(mineral_protocol::PlaylistRef {
        id: mineral_model::PlaylistId::new(mineral_model::SourceKind::NETEASE, "playlist"),
        name: "playlist".to_owned(),
    });
    manager.admit_song(&queued_song, origin, Lane::Playlist);
    let queued = manager
        .snapshot()
        .first()
        .ok_or_eyre("queued row missing")?
        .id
        .clone();
    manager.admit_song(&song("active-a"), DownloadOrigin::Direct, Lane::Direct);
    manager.admit_song(&song("active-b"), DownloadOrigin::Direct, Lane::Direct);
    let first = manager.take_next().ok_or_eyre("first active row missing")?;
    let second = manager
        .take_next()
        .ok_or_eyre("second active row missing")?;
    let mut history = Vec::new();
    for index in 0..102 {
        let song = song(&format!("history-{index}"));
        manager.admit_song(&song, DownloadOrigin::Direct, Lane::Direct);
        let id = manager
            .snapshot()
            .into_iter()
            .find(|row| row.song.id == song.id)
            .ok_or_eyre("history row missing")?
            .id;
        history.push(id);
    }
    for id in history.iter().rev() {
        manager.stop(id)?;
    }
    let expected = [first.id.clone(), second.id.clone(), queued]
        .into_iter()
        .chain(history.iter().take(/*n*/ 100).cloned())
        .collect::<Vec<_>>();
    let actual = manager
        .snapshot()
        .into_iter()
        .map(|row| row.id)
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "历史顺序按完成先后，保留最近 100 条");
    let evicted = history.last().ok_or_eyre("history is empty")?;
    assert!(manager.stop(evicted).is_err(), "已淘汰身份应视为未知");
    manager.stop(&first.id)?;
    manager.finish_attempt(&first, Err(eyre!("writer stopped")));
    manager.stop(&second.id)?;
    manager.finish_attempt(&second, Err(eyre!("writer stopped")));
    Ok(())
}

/// Stop between file commit and row settlement preserves the committed export and success state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_after_export_commit_preserves_download() -> color_eyre::Result<()> {
    let contents = b"downloaded-media".to_vec();
    let url = serve_once(contents.clone()).await?;
    let playback = PlaybackRegistry::new(vec![Arc::new(UrlChannel { url })])?;
    let (manager, _dir) = manager(/*max_concurrent*/ 1, playback).await?;
    manager.admit_song(&song("committed"), DownloadOrigin::Direct, Lane::Direct);
    let attempt = manager.take_next().ok_or_eyre("download not admitted")?;
    let runtime = &manager.inner.runtime;
    let music_dir = runtime
        .music_dir
        .as_deref()
        .ok_or_eyre("export root missing")?;
    let reporter = manager.clone();
    let id = attempt.id.clone();
    let outcome = download_song(
        &runtime.playback,
        &DownloadEnv {
            music_dir,
            hooks: &runtime.hooks,
        },
        &attempt.song,
        attempt.quality,
        DownloadAttempt {
            id: &attempt.id,
            cancellation: &attempt.cancellation,
        },
        Arc::new(move |update| reporter.report(&id, update)),
        runtime.speed_tick,
    )
    .await?;
    let DownloadOutcome::Downloaded { ref path, .. } = outcome else {
        return Err(eyre!("expected a committed export"));
    };
    let path = path.clone();
    manager.stop(&attempt.id)?;
    manager.finish_attempt(&attempt, Ok(outcome));
    manager.stop(&attempt.id)?;
    assert_eq!(
        row(&manager, &attempt.id)?.status,
        DownloadStatus::Downloaded
    );
    assert_eq!(std::fs::read(path)?, contents, "Stop 不删除已提交文件");
    assert_eq!(manager.summary().active, 0);
    Ok(())
}
