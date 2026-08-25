//! Playback hook behavior across playable, no-direct, unplayable, and prefetch plans.

use super::*;
use pretty_assertions::assert_eq;

/// Starts an explicit test playback.
fn play(core: &PlayerCore, target: &Song) {
    core.play_song(
        target,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
}

/// A playable plan without direct media remains playable in the hook context.
#[tokio::test]
async fn playable_without_direct_is_not_reported_unplayable() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script_playback(
        r#"
            mineral.hook("before_stream", function(ctx)
                if ctx.unplayable then
                    return { skip = "wrongly classified" }
                end
            end)
            "#,
        crate::StatsRecorder::disabled(),
        Duration::ZERO,
        /*fail*/ false,
        /*direct*/ false,
    )?;
    let target = song("a");
    play(&core, &target);
    let opened = wait_until(|| {
        core.with_state(|state| {
            state
                .media_info
                .as_ref()
                .is_some_and(|info| info.song_id == target.id)
                && state.direct_media.is_none()
        })
    })
    .await;
    assert!(
        opened,
        "no-direct prepared media should still open and become current"
    );
    drop(runtime);
    Ok(())
}

/// A hook rewrite replaces the direct locator and final media facts before open.
#[tokio::test]
async fn rewrite_replaces_direct_media_before_open() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let replacement = dir.path().join("replacement.mp3");
    std::fs::write(&replacement, b"replacement")?;
    let script = format!(
        r#"
            mineral.hook("before_stream", function(ctx)
                return {{ url = "file://{}", quality = "standard" }}
            end)
            "#,
        replacement.display()
    );
    let (core, runtime) = core_with_script(&script)?;
    let target = song("a");
    play(&core, &target);
    let rewritten = wait_until(|| {
        core.with_state(|state| {
            state
                .media_info
                .as_ref()
                .is_some_and(|info| info.quality == BitRate::Standard && info.substituted)
                && state.direct_media.as_ref().is_some_and(|media| {
                    media.locator().local_path() == Some(replacement.as_path())
                })
        })
    })
    .await;
    assert!(
        rewritten,
        "rewritten local direct media should be opened and committed"
    );
    drop(runtime);
    Ok(())
}

/// Immediate Skip advances to the next queue item and invalidates the skipped attempt.
#[tokio::test]
async fn immediate_skip_advances_to_next() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            local skipped = false
            mineral.hook("before_stream", function(ctx)
                if not skipped then
                    skipped = true
                    return false
                end
            end)
            "#,
    )?;
    core.with_state(|state| {
        state.queue = vec![song("a"), song("b")];
        state.cursor = mineral_protocol::PlayCursor::InQueue(0);
    });
    play(&core, &song("a"));
    let advanced = wait_until(|| {
        core.with_state(|state| {
            state
                .current_song
                .as_ref()
                .is_some_and(|current| current.id == song("b").id)
        })
    })
    .await;
    assert!(advanced, "Skip should advance to the next queue occurrence");
    drop(runtime);
    Ok(())
}

/// An unplayable provider may be rescued once by a hook-supplied direct replacement.
#[tokio::test]
async fn unplayable_rewrite_opens_fallback() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let replacement = dir.path().join("rescue.m4a");
    std::fs::write(&replacement, b"rescue")?;
    let script = format!(
        r#"
            mineral.hook("before_stream", function(ctx)
                if ctx.unplayable then
                    return {{ url = "file://{}", quality = "higher" }}
                end
            end)
            "#,
        replacement.display()
    );
    let (core, runtime) = core_with_script_playback(
        &script,
        crate::StatsRecorder::disabled(),
        Duration::ZERO,
        /*fail*/ true,
        /*direct*/ true,
    )?;
    let target = song("a");
    play(&core, &target);
    let rescued = wait_until(|| {
        core.with_state(|state| {
            state
                .direct_media
                .as_ref()
                .is_some_and(|media| media.locator().local_path() == Some(replacement.as_path()))
        })
    })
    .await;
    assert!(
        rescued,
        "unplayable hook rewrite should rescue the current attempt"
    );
    drop(runtime);
    Ok(())
}

/// Prefetch Continue arms the same playback instance and its final direct facts.
#[tokio::test]
async fn prefetch_continue_arms_opened_media() -> color_eyre::Result<()> {
    let (core, runtime) =
        core_with_script(r#"mineral.hook("before_stream", function(ctx) return nil end)"#)?;
    let next = song("b");
    let slot = crate::playback_instance::PlaybackSlot::new(next.id.clone());
    core.with_state(|state| {
        state.queue = vec![song("a"), next.clone()];
        state.cursor = mineral_protocol::PlayCursor::InQueue(0);
        state.current_song = Some(song("a"));
        state.prefetch.replace_opening(slot.clone());
    });
    crate::playback::start_prefetch(&core, next.clone(), slot.clone());
    let armed = wait_until(|| {
        core.with_state(|state| {
            let same_instance = state
                .prefetch
                .slot()
                .is_some_and(|active| active.instance_id == slot.instance_id);
            same_instance
                && state.prefetch.queued_mut().is_some_and(|queued| {
                    queued.song.id == next.id && queued.direct_media.is_some()
                })
        })
    })
    .await;
    assert!(
        armed,
        "prefetch should arm the opened media under the same instance id"
    );
    drop(runtime);
    Ok(())
}

/// Prefetch Skip vetoes only the predicted queue occurrence and does not arm audio.
#[tokio::test]
async fn prefetch_skip_vetoes_predicted_occurrence() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx)
                if ctx.mode == "prefetch" then
                    return { skip = "not next" }
                end
            end)
            "#,
    )?;
    let next = song("b");
    let slot = crate::playback_instance::PlaybackSlot::new(next.id.clone());
    core.with_state(|state| {
        state.queue = vec![song("a"), next.clone(), song("c")];
        state.cursor = mineral_protocol::PlayCursor::InQueue(0);
        state.current_song = Some(song("a"));
        state.prefetch.replace_opening(slot.clone());
    });
    crate::playback::start_prefetch(&core, next, slot);
    let vetoed = wait_until(|| core.with_state(|state| state.prefetch_vetoed == vec![1])).await;
    assert!(
        vetoed,
        "prefetch Skip should record the predicted queue index"
    );
    core.with_state(|state| {
        assert!(!state.prefetch.is_armed());
        assert!(state.prefetch.slot().is_none());
        assert_eq!(crate::queue::next_index(state), Some(2));
    });
    drop(runtime);
    Ok(())
}

/// A completed before-stream decision records both resolution and hook system events.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn before_stream_records_hook_fire() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let (core, runtime) = core_with_script_stats(
        r#"mineral.hook("before_stream", function(ctx) end)"#,
        recorder,
    )?;
    play(&core, &song("a"));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while store.status().await?.events < 2 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("timed out waiting for stream resolution and hook fire");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(runtime);
    Ok(())
}

/// 预取被 hook 改写:prefetches 应留 Rewritten → Armed 裁决序列(连
/// stream_resolutions(Ok) 与 hook_fires(Rewrite) 共 4 条事件)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_rewrite_records_rewritten_then_armed() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let replacement = dir.path().join("replacement.mp3");
    std::fs::write(&replacement, b"replacement")?;
    let script = format!(
        r#"
            mineral.hook("before_stream", function(ctx)
                if ctx.mode == "prefetch" then
                    return {{ url = "file://{}", quality = "standard" }}
                end
            end)
            "#,
        replacement.display()
    );
    let (core, runtime) = core_with_script_playback(
        &script,
        recorder,
        Duration::ZERO,
        /*fail*/ false,
        /*direct*/ true,
    )?;
    let next = song("b");
    let slot = crate::playback_instance::PlaybackSlot::new(next.id.clone());
    core.with_state(|state| {
        state.queue = vec![song("a"), next.clone()];
        state.cursor = mineral_protocol::PlayCursor::InQueue(0);
        state.current_song = Some(song("a"));
        state.prefetch.replace_opening(slot.clone());
    });
    crate::playback::start_prefetch(&core, next, slot);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while store.status().await?.events < 4 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:Rewritten / Armed 裁决未落库");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(runtime);
    Ok(())
}
