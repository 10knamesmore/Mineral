//! 配置宿主(覆盖合成 / 校验剔除 / 标题覆盖)· terminal 属性上报 · queue 插入编辑。

use super::*;
use pretty_assertions::assert_eq;

/// 造一个带 event hub 接收端的 core(配置宿主 / 属性下发断言用)。
fn core_with_hub() -> color_eyre::Result<(
    PlayerCore,
    tokio::sync::broadcast::Receiver<mineral_protocol::Event>,
)> {
    let (events_tx, events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 16);
    let core = core_with_events(
        Vec::new(),
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        events_tx,
        /*script*/ None,
    )?;
    Ok((core, events_rx))
}

/// 从 hub 收一帧 `ConfigChanged`,按 JSON pointer 取叶子值(断言辅助)。
fn config_leaf(
    events_rx: &mut tokio::sync::broadcast::Receiver<mineral_protocol::Event>,
    pointer: &str,
) -> color_eyre::Result<serde_json::Value> {
    match events_rx.try_recv()? {
        mineral_protocol::Event::ConfigChanged { config } => config
            .into_json()
            .pointer(pointer)
            .cloned()
            .ok_or_else(|| color_eyre::eyre::eyre!("有效树缺 {pointer}")),
        other => color_eyre::eyre::bail!("应收 ConfigChanged,实得 {other:?}"),
    }
}

/// 配置覆盖:合成 + 校验 + 推送;同值重写与撤销不存在的 path 都不发事件;
/// 撤销回落底树值。
#[tokio::test]
async fn config_override_merges_and_diffs() -> color_eyre::Result<()> {
    use mineral_protocol::BusValue;
    let (core, mut events_rx) = core_with_hub()?;
    core.apply_config_override(
        "tui.lyrics.fullscreen_line_gap".to_owned(),
        Some(BusValue::Int(2)),
    );
    assert_eq!(
        config_leaf(&mut events_rx, "/tui/lyrics/fullscreen_line_gap")?,
        serde_json::json!(2),
        "覆盖合成进有效树"
    );
    // 同值重写:不发。
    core.apply_config_override(
        "tui.lyrics.fullscreen_line_gap".to_owned(),
        Some(BusValue::Int(2)),
    );
    assert!(events_rx.try_recv().is_err(), "同值重写不得重复下发");
    // 撤销不存在的 path:不发。
    core.apply_config_override("tui.lyrics.compact_line_gap".to_owned(), None);
    assert!(events_rx.try_recv().is_err(), "撤销不存在的 path 不得下发");
    // 握手重放快照反映覆盖。
    assert_eq!(
        core.effective_config()
            .into_json()
            .pointer("/tui/lyrics/fullscreen_line_gap"),
        Some(&serde_json::json!(2)),
        "重放快照带覆盖"
    );
    // 真撤销:回落底树默认值(default.lua 的 1)。
    core.apply_config_override("tui.lyrics.fullscreen_line_gap".to_owned(), None);
    assert_eq!(
        config_leaf(&mut events_rx, "/tui/lyrics/fullscreen_line_gap")?,
        serde_json::json!(1),
        "撤销回落底树值"
    );
    Ok(())
}

/// 坏覆盖(类型不符 / 未知路径)被剔除:警告 toast、不推 ConfigChanged、
/// 有效配置保持校验通过的那份;好覆盖不被殃及。
#[tokio::test]
async fn bad_config_override_evicted_with_warning() -> color_eyre::Result<()> {
    use mineral_protocol::{BusValue, Event, ToastKind};
    let (core, mut events_rx) = core_with_hub()?;
    // 先落一条好覆盖。
    core.apply_config_override(
        "tui.lyrics.fullscreen_line_gap".to_owned(),
        Some(BusValue::Int(3)),
    );
    let _ = config_leaf(&mut events_rx, "/tui/lyrics/fullscreen_line_gap")?;
    // 类型不符:剔除 + 警告,好覆盖仍在。
    core.apply_config_override(
        "tui.lyrics.compact_line_gap".to_owned(),
        Some(BusValue::Str("x".to_owned())),
    );
    match events_rx.try_recv()? {
        Event::Toast { kind, .. } => assert_eq!(kind, ToastKind::Warn, "坏覆盖应警告"),
        other => color_eyre::eyre::bail!("应收警告 toast,实得 {other:?}"),
    }
    assert!(
        events_rx.try_recv().is_err(),
        "坏覆盖不得推 ConfigChanged(有效树没变)"
    );
    let effective = core.effective_config().into_json();
    assert_eq!(
        effective.pointer("/tui/lyrics/fullscreen_line_gap"),
        Some(&serde_json::json!(3)),
        "好覆盖不被殃及"
    );
    assert_eq!(
        effective.pointer("/tui/lyrics/compact_line_gap"),
        Some(&serde_json::json!(0)),
        "坏覆盖不生效,保持底树值"
    );
    // 未知路径:deny_unknown_fields 拒 → 剔除 + 警告。
    core.apply_config_override("tui.lyrics.bogus".to_owned(), Some(BusValue::Int(1)));
    match events_rx.try_recv()? {
        Event::Toast { kind, .. } => assert_eq!(kind, ToastKind::Warn, "未知路径应警告"),
        other => color_eyre::eyre::bail!("应收警告 toast,实得 {other:?}"),
    }
    assert!(
        events_rx.try_recv().is_err(),
        "未知路径不得推 ConfigChanged"
    );
    Ok(())
}

/// 换底树(配置文件重载)后 session 覆盖仍叠在新底树上。
#[tokio::test]
async fn set_config_base_reapplies_overlay() -> color_eyre::Result<()> {
    use mineral_protocol::BusValue;
    let (core, mut events_rx) = core_with_hub()?;
    core.apply_config_override(
        "tui.lyrics.fullscreen_line_gap".to_owned(),
        Some(BusValue::Int(4)),
    );
    let _ = config_leaf(&mut events_rx, "/tui/lyrics/fullscreen_line_gap")?;
    // 新底树 = 默认树上改 audio.volume(模拟用户改文件)。
    let new_base = mineral_config::merge_tree(
        mineral_config::default_tree()?,
        serde_json::json!({ "audio": { "volume": 55 } }),
    );
    core.set_config_base(new_base);
    let effective = match events_rx.try_recv()? {
        mineral_protocol::Event::ConfigChanged { config } => config.into_json(),
        other => color_eyre::eyre::bail!("应收 ConfigChanged,实得 {other:?}"),
    };
    assert_eq!(
        effective.pointer("/audio/volume"),
        Some(&serde_json::json!(55)),
        "新底树生效"
    );
    assert_eq!(
        effective.pointer("/tui/lyrics/fullscreen_line_gap"),
        Some(&serde_json::json!(4)),
        "session 覆盖在重载后仍生效"
    );
    Ok(())
}

/// 窗口标题覆盖:直通转发 + 同值 diff + 重放快照;撤销发 None。
#[tokio::test]
async fn window_title_override_forwards_and_diffs() -> color_eyre::Result<()> {
    use mineral_protocol::Event;
    let (core, mut events_rx) = core_with_hub()?;
    core.apply_window_title_override(Some("⏸ 歌名".to_owned()));
    assert_eq!(
        events_rx.try_recv()?,
        Event::WindowTitleOverride {
            text: Some("⏸ 歌名".to_owned()),
        }
    );
    core.apply_window_title_override(Some("⏸ 歌名".to_owned()));
    assert!(events_rx.try_recv().is_err(), "同值重写不得重复下发");
    assert_eq!(
        core.window_title_override().as_deref(),
        Some("⏸ 歌名"),
        "握手重放快照"
    );
    core.apply_window_title_override(None);
    assert_eq!(
        events_rx.try_recv()?,
        Event::WindowTitleOverride { text: None }
    );
    assert_eq!(core.window_title_override(), None, "撤销后无重放");
    Ok(())
}

/// terminal 属性:上报后 check_props 下发 Table,断开清除后回 None。
#[tokio::test]
async fn terminal_prop_follows_report_and_clear() -> color_eyre::Result<()> {
    use mineral_protocol::Event;
    let (core, mut events_rx) = core_with_hub()?;
    core.set_terminal_state(crate::props::TerminalReport {
        rows: 50,
        cols: 220,
        fullscreen: true,
        focused: true,
    });
    core.check_props();
    let terminal_of = |rx: &mut tokio::sync::broadcast::Receiver<Event>| {
        // check_props 首轮全量产出,滤出 terminal 一项。
        let mut found = None;
        while let Ok(ev) = rx.try_recv() {
            if let Event::PropertyChanged { prop, value } = ev
                && prop == mineral_protocol::PropName::TERMINAL
            {
                found = Some(value);
            }
        }
        found
    };
    assert_eq!(
        terminal_of(&mut events_rx),
        Some(mineral_protocol::PropValue::Table(vec![
            ("rows".to_owned(), mineral_protocol::PropValue::Int(50)),
            ("cols".to_owned(), mineral_protocol::PropValue::Int(220)),
            (
                "fullscreen".to_owned(),
                mineral_protocol::PropValue::Bool(true)
            ),
            (
                "focused".to_owned(),
                mineral_protocol::PropValue::Bool(true)
            ),
        ]))
    );
    // 值不变:下一 tick 不再下发。
    core.check_props();
    assert_eq!(terminal_of(&mut events_rx), None, "同值不得重复下发");
    // 断开清除:回 None。
    core.clear_terminal_state();
    core.check_props();
    assert_eq!(
        terminal_of(&mut events_rx),
        Some(mineral_protocol::PropValue::None),
        "断开后 terminal 属性应回 None"
    );
    Ok(())
}

/// 插播插到当前位置后、追加进末尾,当前位置不动;shuffle 下 original_queue 同步。
#[tokio::test]
async fn queue_insert_next_and_append_keep_current() -> color_eyre::Result<()> {
    let core = core_with(Arc::default())?;
    core.set_queue(
        vec![song("a"), song("b")],
        &SongId::new(SourceKind::NETEASE, "a"),
    );
    core.queue_insert_next(song("c"));
    core.queue_append(song("d"));
    {
        let st = core.inner.state.lock();
        let ids = st
            .queue
            .iter()
            .map(|s| s.id.as_str().to_owned())
            .collect::<Vec<String>>();
        assert_eq!(ids, ["a", "c", "b", "d"]);
        assert_eq!(st.queue_sel, 0);
    }
    core.set_play_mode(PlayMode::Shuffle);
    core.queue_insert_next(song("e"));
    {
        let st = core.inner.state.lock();
        let orig = st
            .original_queue
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("shuffle 后应有 original_queue"))?;
        assert!(
            orig.iter().any(|s| s.id.as_str() == "e"),
            "original_queue 应同步插入"
        );
        assert!(st.queue.iter().any(|s| s.id.as_str() == "e"));
    }
    Ok(())
}
