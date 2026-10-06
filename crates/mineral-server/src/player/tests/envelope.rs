//! Envelope orchestration for persistent original media and instance-only hook replacements.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::persistence::ServerStore;
use mineral_model::{AudioFormat, BitRate, Envelope, Song, SongId, SourceKind};
use mineral_protocol::PlayerVersions;
use mineral_test::song;

use super::backends::RecordingChannel;
use super::fixtures::{core_with_channels_music_dir, core_with_script_persist};
use super::waiting::wait_until;
use crate::media_cache::MediaCache;
use crate::playback_instance::PlaybackSlot;
use crate::player::PlayerCore;

/// 把一首真实 WAV 写入歌曲元数据派生的下载路径。
fn put_wav_download(root: &Path, s: &Song) -> color_eyre::Result<()> {
    let (subdir, file_name) =
        crate::media_cache::library_relpath(s, BitRate::Lossless, Some(&AudioFormat::Wav));
    let path = root.join(subdir).join(file_name);
    let parent = path
        .parent()
        .ok_or_else(|| color_eyre::eyre::eyre!("download fixture has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let samples = vec![8_000i16; 8_000];
    mineral_test::write_wav(&path, &samples, 1, 8_000)?;
    Ok(())
}

/// 轮询 `sync` 直到当前曲段(`CurrentSync`)携带包络(或超时),返回 (当前曲 id, 包络)。
/// `known = 0` 让 current 段恒出,包络就绪(adopt bump 版本)后即随段到达。
async fn wait_envelope(core: &PlayerCore) -> color_eyre::Result<(SongId, Envelope)> {
    for _ in 0..200 {
        if let Some(c) = core.sync(PlayerVersions::default()).current
            && let (Some(song), Some(envelope)) = (c.current_song, c.current_envelope)
        {
            return Ok((song.id, envelope));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    color_eyre::eyre::bail!("超时未等到当前曲段携带包络");
}

/// Starts explicit playback through the production song entry point.
fn play(core: &PlayerCore, target: &Song) {
    core.play_song(
        target,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
}

/// Builds the expected envelope for a silent replacement using configured point count.
fn silent_envelope(core: &PlayerCore) -> Envelope {
    Envelope {
        points: vec![0; core.inner.envelope_params.point_count().get()],
        version: mineral_audio::ENVELOPE_VERSION,
    }
}

/// 播放命中本地下载导出 → 包络离线算出、落库、落进当前曲 slot 随 `CurrentSync` 送达。
#[tokio::test(flavor = "multi_thread")]
async fn local_hit_computes_and_pushes_envelope() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let persist = ServerStore::open(&d.path().join("t.db")).await?;
    let root = d.path().join("music");
    let s = song("1");
    put_wav_download(&root, &s)?;
    let core = core_with_channels_music_dir(
        vec![Arc::new(RecordingChannel::default())],
        persist.clone(),
        MediaCache::disabled(),
        &root,
    )?;

    core.play_song(
        &s,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    let (id, envelope) = wait_envelope(&core).await?;
    assert_eq!(id, s.id);
    assert_eq!(
        envelope.points.len(),
        *crate::config::DaemonConfig::defaults()?
            .audio()
            .envelope()
            .points(),
        "现算包络应为配置的点数"
    );
    assert_eq!(
        persist
            .scope(SourceKind::NETEASE)
            .get_envelope(&s.id, envelope.version)
            .await?,
        Some(envelope),
        "包络应落库,重启后可直取"
    );
    Ok(())
}

/// db 已有当前版本包络:开播直推缓存数据(以点数指纹区分),不重复解码。
#[tokio::test(flavor = "multi_thread")]
async fn db_hit_pushes_cached_envelope_without_recompute() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let persist = ServerStore::open(&d.path().join("t.db")).await?;
    let root = d.path().join("music");
    let s = song("1");
    put_wav_download(&root, &s)?;
    // 预置指纹包络(3 点,与现算的 200 点可区分)。
    let fingerprint = Envelope {
        points: vec![1, 2, 3],
        version: mineral_audio::ENVELOPE_VERSION,
    };
    persist
        .scope(SourceKind::NETEASE)
        .put_envelope(&s.id, &fingerprint)
        .await?;
    let core = core_with_channels_music_dir(
        vec![Arc::new(RecordingChannel::default())],
        persist,
        MediaCache::disabled(),
        &root,
    )?;

    core.play_song(
        &s,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    let (id, envelope) = wait_envelope(&core).await?;
    assert_eq!(id, s.id);
    assert_eq!(envelope, fingerprint, "db 命中应直推缓存数据,不重算");
    Ok(())
}

/// Replacements neither load an existing original envelope nor create a missing cache row.
#[tokio::test(flavor = "multi_thread")]
async fn replacement_envelope_is_instance_only() -> color_eyre::Result<()> {
    let fingerprint = Envelope {
        points: vec![1, 2, 3],
        version: mineral_audio::ENVELOPE_VERSION,
    };
    for cached in [None, Some(fingerprint)] {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("server.db")).await?;
        let replacement = dir.path().join("replacement.wav");
        mineral_test::write_wav(&replacement, &vec![0i16; 8_000], 1, 8_000)?;
        let target = song("replacement");
        if let Some(envelope) = &cached {
            persist
                .scope(target.source())
                .put_envelope(&target.id, envelope)
                .await?;
        }
        let script = format!(
            r#"
                mineral.hook("before_stream", function(ctx)
                    return {{ url = "file://{}" }}
                end)
                "#,
            replacement.display()
        );
        let (core, runtime) = core_with_script_persist(&script, persist.clone())?;
        play(&core, &target);
        let (id, envelope) = wait_envelope(&core).await?;
        assert_eq!(id, target.id);
        assert_eq!(envelope, silent_envelope(&core));
        assert!(core.with_state(|state| {
            state
                .media_info
                .as_ref()
                .is_some_and(|info| info.substituted)
        }));
        assert_eq!(
            persist
                .scope(target.source())
                .get_envelope(&target.id, envelope.version)
                .await?,
            cached
        );

        core.refresh_initial_loads();
        let reconnect = core
            .sync(PlayerVersions::default())
            .current
            .ok_or_else(|| color_eyre::eyre::eyre!("missing reconnect current section"))?;
        assert_eq!(reconnect.current_song.map(|song| song.id), Some(target.id));
        assert_eq!(reconnect.current_envelope, Some(envelope));
        drop(runtime);
    }
    Ok(())
}

/// A replacement with failed decoding cannot replay the original cache on client reconnect.
#[tokio::test(flavor = "multi_thread")]
async fn reconnect_does_not_replay_original_envelope_for_replacement() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("server.db")).await?;
    let replacement = dir.path().join("undecodable.wav");
    tokio::fs::write(&replacement, b"not audio").await?;
    let target = song("replacement");
    let fingerprint = Envelope {
        points: vec![1, 2, 3],
        version: mineral_audio::ENVELOPE_VERSION,
    };
    persist
        .scope(target.source())
        .put_envelope(&target.id, &fingerprint)
        .await?;
    let script = format!(
        r#"
            mineral.hook("before_stream", function(ctx)
                return {{ url = "file://{}" }}
            end)
            "#,
        replacement.display()
    );
    let (core, runtime) = core_with_script_persist(&script, persist.clone())?;
    play(&core, &target);
    assert!(
        wait_until(|| core.with_state(|state| {
            state
                .media_info
                .as_ref()
                .is_some_and(|info| info.substituted)
        }))
        .await
    );
    core.refresh_initial_loads();
    assert!(
        !wait_until(|| {
            core.sync(PlayerVersions::default())
                .current
                .is_some_and(|current| current.current_envelope.is_some())
        })
        .await
    );
    assert_eq!(
        persist
            .scope(target.source())
            .get_envelope(&target.id, fingerprint.version)
            .await?,
        Some(fingerprint)
    );
    drop(runtime);
    Ok(())
}

/// Same-song playback rejects old replacement data and retains normal cache replay.
#[tokio::test(flavor = "multi_thread")]
async fn same_song_restart_rejects_old_replacement_envelope() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("server.db")).await?;
    let replacement = dir.path().join("replacement.wav");
    mineral_test::write_wav(&replacement, &vec![0i16; 8_000], 1, 8_000)?;
    let target = song("same-song");
    let fingerprint = Envelope {
        points: vec![1, 2, 3],
        version: mineral_audio::ENVELOPE_VERSION,
    };
    persist
        .scope(target.source())
        .put_envelope(&target.id, &fingerprint)
        .await?;
    let script = format!(
        r#"
            local replaced = false
            mineral.hook("before_stream", function(ctx)
                if not replaced then
                    replaced = true
                    return {{ url = "file://{}" }}
                end
            end)
            "#,
        replacement.display()
    );
    let (core, runtime) = core_with_script_persist(&script, persist)?;
    play(&core, &target);
    let (_, replacement_envelope) = wait_envelope(&core).await?;
    assert_eq!(replacement_envelope, silent_envelope(&core));
    let old_slot = core
        .with_state(|state| state.current_slot.clone())
        .ok_or_else(|| color_eyre::eyre::eyre!("missing replacement slot"))?;

    play(&core, &target);
    let current = core
        .sync(PlayerVersions::default())
        .current
        .ok_or_else(|| color_eyre::eyre::eyre!("missing restarted current section"))?;
    assert_ne!(current.current_envelope, Some(replacement_envelope.clone()));
    // The completion callback uses this adoption boundary after async decoding.
    assert!(!core.with_state(|state| {
        state.adopt_envelope(
            old_slot.instance_id,
            &target.id,
            replacement_envelope.clone(),
        )
    }));
    assert!(
        wait_until(|| core.with_state(|state| {
            state
                .media_info
                .as_ref()
                .is_some_and(|info| !info.substituted)
        }))
        .await
    );
    core.replay_current_envelope();
    assert_eq!(wait_envelope(&core).await?.1, fingerprint);
    assert!(!core.with_state(|state| {
        state.adopt_envelope(old_slot.instance_id, &target.id, replacement_envelope)
    }));
    assert_eq!(wait_envelope(&core).await?.1, fingerprint);
    drop(runtime);
    Ok(())
}

/// Same-song prefetch retains its own envelope across promotion and later instance callbacks.
#[tokio::test(flavor = "multi_thread")]
async fn prefetched_replacement_envelope_follows_gapless_instance() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("server.db")).await?;
    let replacement = dir.path().join("replacement.wav");
    mineral_test::write_wav(&replacement, &vec![0i16; 8_000], 1, 8_000)?;
    let target = song("same-song");
    let fingerprint = Envelope {
        points: vec![1, 2, 3],
        version: mineral_audio::ENVELOPE_VERSION,
    };
    persist
        .scope(target.source())
        .put_envelope(&target.id, &fingerprint)
        .await?;
    let script = format!(
        r#"
            mineral.hook("before_stream", function(ctx)
                if ctx.mode == "prefetch" then
                    return {{ url = "file://{}" }}
                end
            end)
            "#,
        replacement.display()
    );
    let (core, runtime) = core_with_script_persist(&script, persist.clone())?;
    play(&core, &target);
    assert!(wait_until(|| core.with_state(|state| state.media_info.is_some())).await);
    core.replay_current_envelope();
    assert_eq!(wait_envelope(&core).await?.1, fingerprint);
    let old_slot = core
        .with_state(|state| state.current_slot.clone())
        .ok_or_else(|| color_eyre::eyre::eyre!("missing original slot"))?;
    let next_slot = PlaybackSlot::new(target.id.clone());
    core.with_state(|state| {
        state.queue = vec![target.clone(), target.clone()];
        state.cursor = mineral_protocol::PlayCursor::InQueue(0);
        state.prefetch.replace_opening(next_slot.clone());
    });
    crate::playback::start_prefetch(&core, target.clone(), next_slot.clone());
    assert!(
        wait_until(|| core.with_state(|state| {
            state.prefetch.is_armed()
                && state
                    .prefetch
                    .slot()
                    .is_some_and(|slot| slot.envelope.is_some())
        }))
        .await
    );
    assert_eq!(wait_envelope(&core).await?.1, fingerprint);

    assert_eq!(
        core.with_state(crate::gapless::adopt_queued),
        Some(target.id.clone())
    );
    assert_eq!(
        core.with_state(|state| state.current_slot.as_ref().map(|slot| slot.instance_id)),
        Some(next_slot.instance_id)
    );
    let envelope = silent_envelope(&core);
    assert_eq!(wait_envelope(&core).await?.1, envelope);
    // A computation finishing after promotion still owns the same slot.
    assert!(core.with_state(|state| {
        state.adopt_envelope(next_slot.instance_id, &target.id, envelope.clone())
    }));
    assert!(!core.with_state(|state| {
        state.adopt_envelope(old_slot.instance_id, &target.id, fingerprint.clone())
    }));
    core.refresh_initial_loads();
    assert_eq!(wait_envelope(&core).await?.1, envelope);
    assert_eq!(
        persist
            .scope(target.source())
            .get_envelope(&target.id, fingerprint.version)
            .await?,
        Some(fingerprint)
    );
    drop(runtime);
    Ok(())
}
