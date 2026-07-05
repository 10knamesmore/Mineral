//! UI 覆盖下发 · terminal 属性上报 · queue 插入编辑。

use super::*;
use pretty_assertions::assert_eq;

/// 造一个带 event hub 接收端的 core(UI 覆盖 / 属性下发断言用)。
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

/// apply_ui_override:存表 + 转发;同值重写与撤销不存在的 key 都不发事件。
#[tokio::test]
async fn ui_override_stores_forwards_and_diffs() -> color_eyre::Result<()> {
    use mineral_protocol::{BusValue, Event};
    let (core, mut events_rx) = core_with_hub()?;
    core.apply_ui_override(
        "lyrics.fullscreen_line_gap".to_owned(),
        Some(BusValue::Int(2)),
    );
    assert_eq!(
        events_rx.try_recv()?,
        Event::UiOverride {
            key: "lyrics.fullscreen_line_gap".to_owned(),
            value: Some(BusValue::Int(2)),
        }
    );
    // 同值重写:不发。
    core.apply_ui_override(
        "lyrics.fullscreen_line_gap".to_owned(),
        Some(BusValue::Int(2)),
    );
    assert!(events_rx.try_recv().is_err(), "同值重写不得重复下发");
    // 撤销不存在的 key:不发。
    core.apply_ui_override("no.such".to_owned(), None);
    assert!(events_rx.try_recv().is_err(), "撤销不存在的 key 不得下发");
    // 快照只含在表的键。
    assert_eq!(
        core.ui_overrides_snapshot(),
        vec![("lyrics.fullscreen_line_gap".to_owned(), BusValue::Int(2))]
    );
    // 真撤销:发 None + 表清空。
    core.apply_ui_override("lyrics.fullscreen_line_gap".to_owned(), None);
    assert_eq!(
        events_rx.try_recv()?,
        Event::UiOverride {
            key: "lyrics.fullscreen_line_gap".to_owned(),
            value: None,
        }
    );
    assert!(core.ui_overrides_snapshot().is_empty(), "撤销后表应为空");
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
