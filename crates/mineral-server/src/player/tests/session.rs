//! session 快照 · save/load 往返 · restore play_mode · 周期保存。

use super::*;
use pretty_assertions::assert_eq;

/// volume_pct(u8 0..=100)→ f64 0.0..=1.0:80 → 0.8。
#[tokio::test]
async fn snapshot_session_converts_volume() -> color_eyre::Result<()> {
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with(calls)?;
    core.audio().set_volume(80);
    let snap = core.snapshot_session();
    assert!((snap.volume - 0.8).abs() < 1e-9, "80% 应映射到 0.8");
    Ok(())
}

/// load_session 空库返回 Ok(None)。
#[tokio::test]
async fn load_session_empty_returns_none() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with_persist(calls, persist)?;
    assert!(core.load_session().await?.is_none(), "空库应读不到会话");
    Ok(())
}

/// 设入队列 + 当前歌 + 模式后,组装的 [`SessionSnapshot`] 落盘再 load 读回内容一致。
///
/// 注:直接 `snapshot_session()` + `session().save()` 落盘(而非依赖 background
/// fire-and-forget 的多次并发 save —— 它们写同一单例行无确定顺序),断言数据正确。
#[tokio::test]
async fn save_then_load_roundtrips_queue_and_current() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with_persist(calls, persist.clone())?;

    core.cycle_play_mode(); // Sequential → Shuffle
    let queue = vec![song("a"), song("b"), song("c")];
    core.set_queue(queue, &song("a").id);
    core.play_song(&song("a"));
    // 组装快照并同步落盘(确定性,不依赖 spawn 顺序)。
    let assembled = core.snapshot_session();
    persist.session().save(&assembled).await?;

    let snap = core
        .load_session()
        .await?
        .ok_or_else(|| color_eyre::eyre::eyre!("应读回会话"))?;
    assert_eq!(snap.queue.len(), 3, "队列长度应为 3");
    assert!(snap.queue.contains(&song("a").id), "队列应含 a");
    assert_eq!(snap.current, Some(song("a").id), "当前歌应为 a");
    assert_eq!(snap.play_mode, "Shuffle", "模式应为 Shuffle");
    Ok(())
}

/// 启动恢复路径:落库的模式名经 `PlayMode::from_name` 解析 + `restore_play_mode`
/// 写回——只动模式标志,不触发洗牌边界(队列空、original_queue 不被置),不回写会话。
#[tokio::test]
async fn restore_play_mode_sets_flag_without_shuffle_side_effects() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with_persist(calls, persist.clone())?;

    // 模拟上一次会话:Shuffle 模式落盘。
    core.cycle_play_mode(); // Sequential → Shuffle
    persist.session().save(&core.snapshot_session()).await?;

    // 模拟下一次启动:新 core 读回会话,解析模式名并恢复。
    let calls2 = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let fresh = core_with_persist(calls2, persist)?;
    let snap = fresh
        .load_session()
        .await?
        .ok_or_else(|| color_eyre::eyre::eyre!("应读回会话"))?;
    let mode = PlayMode::from_name(&snap.play_mode)
        .ok_or_else(|| color_eyre::eyre::eyre!("落库模式名应可解析: {}", snap.play_mode))?;
    fresh.restore_play_mode(mode);

    let st = fresh.inner.state.lock();
    assert_eq!(st.play_mode, PlayMode::Shuffle, "模式标志应恢复");
    assert!(st.queue.is_empty(), "恢复不带队列");
    assert!(
        st.original_queue.is_none(),
        "restore 不该触发 enter_shuffle 的洗牌/存原序边界"
    );
    Ok(())
}

/// 周期落盘的空态守卫:daemon 空闲(无当前曲、空队列)时跳过,上次会话的队列
/// 不被空快照覆盖——那是将来队列恢复要吃的数据。
#[tokio::test]
async fn periodic_save_skips_empty_state_preserving_last_session() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    // 上次会话:真实队列同步落盘。
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with_persist(calls, persist.clone())?;
    core.set_queue(vec![song("a"), song("b")], &song("a").id);
    persist.session().save(&core.snapshot_session()).await?;

    // 模拟新启动:空态 core,把节流窗口拨到已过期再触发周期检查。
    let calls2 = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let fresh = core_with_persist(calls2, persist)?;
    if let Some(past) = std::time::Instant::now().checked_sub(Duration::from_secs(60)) {
        *fresh.inner.last_session_save.lock() = past;
    }
    fresh.check_session_save();
    drain_spawned().await;

    let snap = fresh
        .load_session()
        .await?
        .ok_or_else(|| color_eyre::eyre::eyre!("应读回会话"))?;
    assert_eq!(snap.queue.len(), 2, "空态周期落盘应跳过,上次队列应保留");
    Ok(())
}

/// fire-and-forget 的 spawn_save_session 最终能让 load 读到会话(不断言精确字段值,
/// 只确认接线打通、数据落盘)。
#[tokio::test]
async fn spawn_save_session_persists_something() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with_persist(calls, persist)?;

    core.set_queue(vec![song("a"), song("b")], &song("a").id);
    drain_spawned().await;

    assert!(core.load_session().await?.is_some(), "save 后应能读到会话");
    Ok(())
}
