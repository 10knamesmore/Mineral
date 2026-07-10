//! 包络编排:本地命中开播 → 离线计算 → 落库 → `EnvelopeReady` 推送;
//! db 命中直推缓存数据不重算。

use std::path::Path;

use mineral_model::{AudioFormat, Envelope};

use super::*;
use crate::media_cache::library_relpath;

/// 把一首歌的真实 WAV 按下载导出布局写进 `root`(恒幅样本,保证可解码出包络)。
fn put_wav_download(root: &Path, s: &Song) -> color_eyre::Result<()> {
    let (subdir, file_name) = library_relpath(s, BitRate::Lossless, Some(&AudioFormat::Wav));
    let abs = root.join(&subdir).join(&file_name);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let samples = vec![8_000i16; 8_000];
    mineral_test::write_wav(
        &abs, &samples, /*channels*/ 1, /*sample_rate*/ 8_000,
    )
}

/// 轮询 drain 直到收到一条 `EnvelopeReady`(或超时),返回其载荷。
async fn wait_envelope(core: &PlayerCore) -> color_eyre::Result<(SongId, Envelope)> {
    for _ in 0..200 {
        for ev in core.drain_client_events() {
            if let mineral_task::TaskEvent::EnvelopeReady { song_id, envelope } = ev {
                return Ok((song_id, envelope));
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    color_eyre::eyre::bail!("超时未收到 EnvelopeReady")
}

/// 播放命中本地下载导出 → 包络离线算出、落库、`EnvelopeReady` 推给 client。
#[tokio::test(flavor = "multi_thread")]
async fn local_hit_computes_and_pushes_envelope() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let persist = ServerStore::open(&d.path().join("t.db")).await?;
    let root = d.path().join("music");
    let s = song("1");
    put_wav_download(&root, &s)?;
    let core = core_with_channels(
        vec![Arc::new(RecordingChannel::default())],
        persist.clone(),
        Some(root),
        MediaCache::disabled(),
    )?;

    core.play_song(&s);
    let (id, envelope) = wait_envelope(&core).await?;
    assert_eq!(id, s.id);
    assert_eq!(
        envelope.points.len(),
        mineral_audio::ENVELOPE_POINT_COUNT,
        "现算包络应为标准点数"
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
    let core = core_with_channels(
        vec![Arc::new(RecordingChannel::default())],
        persist,
        Some(root),
        MediaCache::disabled(),
    )?;

    core.play_song(&s);
    let (id, envelope) = wait_envelope(&core).await?;
    assert_eq!(id, s.id);
    assert_eq!(envelope, fingerprint, "db 命中应直推缓存数据,不重算");
    Ok(())
}
