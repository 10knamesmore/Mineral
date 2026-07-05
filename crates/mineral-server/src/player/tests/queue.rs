//! queue 导航 · shuffle · play_mode · version bump · skip 记录。

use super::*;
use pretty_assertions::assert_eq;

#[test]
fn next_sequential_stops_at_end() {
    assert!(next_in_queue(&state_with(&["a", "b", "c"], 2, PlayMode::Sequential)).is_none());
    assert_eq!(
        next_in_queue(&state_with(&["a", "b", "c"], 0, PlayMode::Sequential)),
        Some(song("b"))
    );
}

/// next:RepeatAll / Shuffle 在尾部环回到首,RepeatOne 原地。
#[test]
fn next_wraps_and_repeats_one() {
    assert_eq!(
        next_in_queue(&state_with(&["a", "b", "c"], 2, PlayMode::RepeatAll)),
        Some(song("a"))
    );
    assert_eq!(
        next_in_queue(&state_with(&["a", "b", "c"], 2, PlayMode::Shuffle)),
        Some(song("a"))
    );
    assert_eq!(
        next_in_queue(&state_with(&["a", "b", "c"], 1, PlayMode::RepeatOne)),
        Some(song("b"))
    );
}

/// prev:Sequential 首位返回 None,否则取上一首的下标。
#[test]
fn prev_sequential_stops_at_start() {
    assert!(prev_index(&state_with(&["a", "b", "c"], 0, PlayMode::Sequential)).is_none());
    assert_eq!(
        prev_index(&state_with(&["a", "b", "c"], 2, PlayMode::Sequential)),
        Some(1) // b
    );
}

/// prev:RepeatAll / Shuffle 在首部环回到尾,RepeatOne 原地(均以下标计)。
#[test]
fn prev_wraps_and_repeats_one() {
    assert_eq!(
        prev_index(&state_with(&["a", "b", "c"], 0, PlayMode::RepeatAll)),
        Some(2) // c
    );
    assert_eq!(
        prev_index(&state_with(&["a", "b", "c"], 0, PlayMode::Shuffle)),
        Some(2) // c
    );
    assert_eq!(
        prev_index(&state_with(&["a", "b", "c"], 1, PlayMode::RepeatOne)),
        Some(1) // b
    );
}

/// 回归:队列含交替重复曲时,顺序推进必须按下标单向前进、走到队尾后停,
/// **不得**在两个重复副本之间来回吸附成无限循环(历史 bug:落地按歌曲身份
/// first-match 定位,重复曲把 queue_sel 拽回首个副本)。
#[test]
fn advance_next_walks_past_duplicates_without_looping() {
    // gk 在下标 1/3、400 在下标 2/5;从正在播的 400(下标 2)起逐首推进。
    let mut st = state_with(
        &["intro", "gk", "400", "gk", "fish", "400", "outro"],
        2,
        PlayMode::Sequential,
    );
    let mut visited = Vec::new();
    while let Some(_song) = advance_next(&mut st) {
        visited.push(st.queue_sel);
    }
    // 2→3→4→5→6 后到队尾停;每步严格 +1,绝不回退到 1 或 2。
    assert_eq!(visited, vec![3, 4, 5, 6]);
    assert_eq!(st.queue_sel, 6, "推进到队尾后 queue_sel 停在末位");
}

/// 回归:advance_prev 同样按下标后退,重复曲不把 queue_sel 吸附到首个副本。
#[test]
fn advance_prev_steps_back_by_index_with_duplicates() {
    let mut st = state_with(&["a", "b", "a", "b"], 3, PlayMode::Sequential); // 第二个 b
    assert_eq!(
        advance_prev(&mut st).as_ref().map(|s| s.id.as_str()),
        Some("a")
    );
    assert_eq!(st.queue_sel, 2, "应退到下标 2(第二个 a),而非首个 a@0");
    assert_eq!(
        advance_prev(&mut st).as_ref().map(|s| s.id.as_str()),
        Some("b")
    );
    assert_eq!(st.queue_sel, 1);
}

/// 空队列时 next / prev 都返回 None。
#[test]
fn empty_queue_has_no_neighbors() {
    assert!(next_in_queue(&State::empty()).is_none());
    assert!(prev_index(&State::empty()).is_none());
}

/// queue_sel 越界被 clamp 到末位:Sequential next=None、prev=倒数第二首。
#[test]
fn out_of_bounds_sel_is_clamped() {
    let st = state_with(&["a", "b"], 5, PlayMode::Sequential);
    assert!(next_in_queue(&st).is_none());
    assert_eq!(prev_index(&st), Some(0)); // a
}

/// enter_shuffle:内容集合不变 + 当前歌置顶 + queue_sel=0 + original 存原序。
#[test]
fn enter_shuffle_keeps_all_and_pins_current() {
    let mut st = state_with(&["a", "b", "c", "d"], 2, PlayMode::Sequential); // current=c
    enter_shuffle(&mut st);
    assert_eq!(st.queue.first().map(|s| s.id.as_str()), Some("c"));
    assert_eq!(st.queue_sel, 0);
    assert_eq!(ids_sorted(&st.queue), vec!["a", "b", "c", "d"]);
    assert_eq!(
        st.original_queue.as_deref().map(ids),
        Some(vec!["a", "b", "c", "d"])
    );
}

/// enter_shuffle:空队列 no-op,不设 original。
#[test]
fn enter_shuffle_empty_is_noop() {
    let mut st = State::empty();
    enter_shuffle(&mut st);
    assert!(st.queue.is_empty());
    assert!(st.original_queue.is_none());
}

/// exit_shuffle:从 original 还原原序,queue_sel 重定位到当前歌,清 original。
#[test]
fn exit_shuffle_restores_order_and_relocates_sel() {
    let mut st = state_with(&["a", "b", "c", "d"], 0, PlayMode::Shuffle);
    st.queue = vec![song("c"), song("a"), song("d"), song("b")];
    st.queue_sel = 0;
    st.current_song = Some(song("c"));
    st.original_queue = Some(vec![song("a"), song("b"), song("c"), song("d")]);
    exit_shuffle(&mut st);
    assert_eq!(ids(&st.queue), vec!["a", "b", "c", "d"]);
    assert_eq!(st.queue_sel, 2); // c 在原序的下标
    assert!(st.original_queue.is_none());
}

/// exit_shuffle:没有 original 时 no-op。
#[test]
fn exit_shuffle_without_original_is_noop() {
    let mut st = state_with(&["a", "b"], 1, PlayMode::Sequential);
    st.original_queue = None;
    exit_shuffle(&mut st);
    assert_eq!(ids(&st.queue), vec!["a", "b"]);
    assert_eq!(st.queue_sel, 1);
}

/// apply_play_mode:目标与当前相同时 no-op。
#[test]
fn apply_same_mode_is_noop() {
    let mut st = state_with(&["a", "b"], 0, PlayMode::Sequential);
    apply_play_mode(&mut st, PlayMode::Sequential);
    assert_eq!(st.play_mode, PlayMode::Sequential);
    assert!(st.original_queue.is_none());
}

/// enter/exit shuffle 必须推进 queue_version(漏 bump = client 永远看不到洗牌结果)。
#[test]
fn shuffle_boundaries_bump_queue_version() {
    let mut st = state_with(&["a", "b", "c"], 1, PlayMode::Sequential);
    let v0 = st.queue_version;
    apply_play_mode(&mut st, PlayMode::Shuffle);
    assert_eq!(st.queue_version, v0 + 1, "进 Shuffle 洗牌后应 bump");
    apply_play_mode(&mut st, PlayMode::Sequential);
    assert_eq!(st.queue_version, v0 + 2, "退 Shuffle 还原后应 bump");
}

/// shuffle 边界的 no-op 路径(空队列进入 / 无 original 退出)不得虚涨版本。
#[test]
fn noop_shuffle_paths_do_not_bump() {
    let mut empty = State::empty();
    let v0 = empty.queue_version;
    enter_shuffle(&mut empty);
    assert_eq!(empty.queue_version, v0, "空队列进 Shuffle 是 no-op");

    let mut st = state_with(&["a", "b"], 1, PlayMode::Sequential);
    let v1 = st.queue_version;
    exit_shuffle(&mut st);
    assert_eq!(st.queue_version, v1, "无 original 退 Shuffle 是 no-op");
}

/// 非 Shuffle 边界的模式切换(RepeatAll → RepeatOne)不动 queue,不得 bump。
#[test]
fn mode_change_without_queue_mutation_does_not_bump() {
    let mut st = state_with(&["a", "b"], 0, PlayMode::RepeatAll);
    let v0 = st.queue_version;
    apply_play_mode(&mut st, PlayMode::RepeatOne);
    assert_eq!(st.queue_version, v0);
}

/// set_queue(两种模式)必须推进 queue_version。
#[tokio::test]
async fn set_queue_bumps_queue_version() -> color_eyre::Result<()> {
    let core = core_with(Arc::default())?;
    let v0 = core.sync(PlayerVersions::default()).versions.queue;
    core.set_queue(
        vec![song("a"), song("b")],
        &SongId::new(SourceKind::NETEASE, "a"),
    );
    let v1 = core.sync(PlayerVersions::default()).versions.queue;
    assert_eq!(v1, v0 + 1, "顺序模式 set_queue 应 bump");

    core.set_play_mode(PlayMode::Shuffle); // 进 Shuffle 本身也 bump 一次
    let v2 = core.sync(PlayerVersions::default()).versions.queue;
    core.set_queue(
        vec![song("c"), song("d")],
        &SongId::new(SourceKind::NETEASE, "c"),
    );
    let v3 = core.sync(PlayerVersions::default()).versions.queue;
    assert_eq!(v3, v2 + 1, "Shuffle 模式 set_queue 应 bump");
    Ok(())
}

/// play_song 清旧上下文 + 写新 current_song,必须推进 current_version。
#[tokio::test]
async fn play_song_bumps_current_version() -> color_eyre::Result<()> {
    let core = core_with(Arc::default())?;
    let v0 = core.sync(PlayerVersions::default()).versions.current;
    core.play_song(&song("a"));
    let v1 = core.sync(PlayerVersions::default()).versions.current;
    assert_eq!(v1, v0 + 1);
    Ok(())
}

/// 回归:play_song 落地时,若 `queue_sel` 已精确指向本曲(顺序推进入口预置好),
/// 不得再按身份 first-match 回溯——否则重复曲会把下标拽回首个副本。
#[tokio::test]
async fn play_song_keeps_preset_queue_sel_on_duplicate() -> color_eyre::Result<()> {
    let core = core_with(Arc::default())?;
    core.with_state(|st| {
        st.queue = vec![song("a"), song("b"), song("a"), song("b")];
        st.queue_sel = 2; // 第二个 a
        st.current_song = Some(song("a"));
    });
    core.play_song(&song("a"));
    core.with_state(|st| {
        assert_eq!(st.queue_sel, 2, "已预置的精确下标须保留,不能吸附到 a@0");
    });
    Ok(())
}

/// play_song 的身份定位仍在:queue_sel 未指向目标曲时,按 first-match 重新定位。
#[tokio::test]
async fn play_song_locates_when_not_preset() -> color_eyre::Result<()> {
    let core = core_with(Arc::default())?;
    core.with_state(|st| {
        st.queue = vec![song("a"), song("b")];
        st.queue_sel = 0;
        st.current_song = Some(song("a"));
    });
    core.play_song(&song("b"));
    core.with_state(|st| assert_eq!(st.queue_sel, 1, "点播未在位的曲应重新定位"));
    Ok(())
}

/// LyricsReady 命中当前歌写入歌词 → bump;不命中丢弃 → 不 bump。
#[tokio::test]
async fn lyrics_ready_bumps_only_on_store() -> color_eyre::Result<()> {
    let core = core_with(Arc::default())?;
    core.with_state(|st| st.current_song = Some(song("a")));
    let v0 = core.sync(PlayerVersions::default()).versions.current;

    core.handle_lyrics_ready(&SongId::new(SourceKind::NETEASE, "x"), Lyrics::default());
    let v1 = core.sync(PlayerVersions::default()).versions.current;
    assert_eq!(v1, v0, "非当前歌的歌词被丢弃,不应 bump");

    core.handle_lyrics_ready(&SongId::new(SourceKind::NETEASE, "a"), Lyrics::default());
    let v2 = core.sync(PlayerVersions::default()).versions.current;
    assert_eq!(v2, v0 + 1, "命中当前歌写入歌词应 bump");
    Ok(())
}

/// PlayUrlReady 命中当前歌写 play_url → bump;不命中任何路由 → 不 bump。
#[tokio::test]
async fn play_url_ready_bumps_only_on_current() -> color_eyre::Result<()> {
    let core = core_with(Arc::default())?;
    core.with_state(|st| st.current_song = Some(song("a")));
    let v0 = core.sync(PlayerVersions::default()).versions.current;

    core.handle_play_url_ready(&SongId::new(SourceKind::NETEASE, "x"), test_play_url("x")?);
    let v1 = core.sync(PlayerVersions::default()).versions.current;
    assert_eq!(v1, v0, "无人认领的 URL 被丢弃,不应 bump");

    core.handle_play_url_ready(&SongId::new(SourceKind::NETEASE, "a"), test_play_url("a")?);
    let v2 = core.sync(PlayerVersions::default()).versions.current;
    assert_eq!(v2, v0 + 1, "命中当前歌写 play_url 应 bump");
    Ok(())
}

/// apply_play_mode:进入 Shuffle 触发 enter(置顶 + 存 original),退回触发 exit(还原)。
#[test]
fn apply_enter_then_exit_shuffle() {
    let mut st = state_with(&["a", "b", "c"], 1, PlayMode::Sequential); // current=b
    apply_play_mode(&mut st, PlayMode::Shuffle);
    assert_eq!(st.play_mode, PlayMode::Shuffle);
    assert!(st.original_queue.is_some());
    assert_eq!(st.queue.first().map(|s| s.id.as_str()), Some("b"));

    apply_play_mode(&mut st, PlayMode::Sequential);
    assert!(st.original_queue.is_none());
    assert_eq!(ids(&st.queue), vec!["a", "b", "c"]);
}

/// apply_play_mode:两个非 Shuffle 模式间切换不动队列、不设 original。
#[test]
fn apply_between_non_shuffle_keeps_queue() {
    let mut st = state_with(&["a", "b", "c"], 1, PlayMode::Sequential);
    apply_play_mode(&mut st, PlayMode::RepeatAll);
    assert_eq!(st.play_mode, PlayMode::RepeatAll);
    assert_eq!(ids(&st.queue), vec!["a", "b", "c"]);
    assert!(st.original_queue.is_none());

    apply_play_mode(&mut st, PlayMode::RepeatOne);
    assert_eq!(ids(&st.queue), vec!["a", "b", "c"]);
    assert!(st.original_queue.is_none());
}

/// next_song(手动跳过):对刚播完的旧歌打 `(old_id, false, position_ms)` 点。
#[tokio::test]
async fn next_song_records_skip_for_old_song() -> color_eyre::Result<()> {
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with(calls.clone())?;
    {
        let mut st = core.inner.state.lock();
        st.queue = vec![song("a"), song("b")];
        st.queue_sel = 0;
        st.current_song = Some(song("a"));
        st.play_mode = PlayMode::Sequential;
    }
    core.next_song();
    drain_spawned().await;

    let recorded = calls.lock().clone();
    assert_eq!(recorded.len(), 1, "应只对旧歌打一次跳过点");
    let (id, completed, _listen) = recorded
        .first()
        .cloned()
        .unwrap_or_else(|| (SongId::new(SourceKind::NETEASE, "missing"), true, u64::MAX));
    assert_eq!(id, song("a").id);
    assert!(!completed, "手动跳过应记 completed=false");
    Ok(())
}

/// next_song:队尾(Sequential)无下一首时不切歌,也不打点。
#[tokio::test]
async fn next_song_at_end_records_nothing() -> color_eyre::Result<()> {
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with(calls.clone())?;
    {
        let mut st = core.inner.state.lock();
        st.queue = vec![song("a"), song("b")];
        st.queue_sel = 1;
        st.current_song = Some(song("b"));
        st.play_mode = PlayMode::Sequential;
    }
    core.next_song();
    drain_spawned().await;

    assert!(calls.lock().is_empty(), "队尾无下一首,不应打点");
    Ok(())
}

/// prev_or_restart:进度 ≤ 阈值真正切到上一首 → 打 `(old_id, false, _)` 点。
#[tokio::test]
async fn prev_below_threshold_records_skip() -> color_eyre::Result<()> {
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with(calls.clone())?;
    {
        let mut st = core.inner.state.lock();
        st.queue = vec![song("a"), song("b")];
        st.queue_sel = 1;
        st.current_song = Some(song("b"));
        st.play_mode = PlayMode::Sequential;
    }
    // ForceNull 起步 position_ms == 0,< 阈值,走「跳上一首」分支。
    core.prev_or_restart();
    drain_spawned().await;

    let recorded = calls.lock().clone();
    assert_eq!(recorded.len(), 1, "应对旧歌打一次跳过点");
    let (id, completed, _listen) = recorded
        .first()
        .cloned()
        .unwrap_or_else(|| (SongId::new(SourceKind::NETEASE, "missing"), true, u64::MAX));
    assert_eq!(id, song("b").id);
    assert!(!completed, "上一首跳过应记 completed=false");
    Ok(())
}

/// play_mode_str:各档落地为稳定 Debug 名。
#[test]
fn play_mode_str_is_debug_name() {
    assert_eq!(PlayMode::Sequential.name(), "Sequential");
    assert_eq!(PlayMode::Shuffle.name(), "Shuffle");
    assert_eq!(PlayMode::RepeatAll.name(), "RepeatAll");
    assert_eq!(PlayMode::RepeatOne.name(), "RepeatOne");
}
