//! 属性树 diff:background_loop 每 tick 采样可观测属性,变更经
//! [`Notifier`](crate::notify::Notifier) 双路下发(订阅 client + 脚本)。
//!
//! 「高频合并只回末值」的语义由 tick 采样天然给出:tick 间无论变多少次,
//! 下游只看到采样时刻的末值。position 只在整秒值变化时产。

use mineral_script::{PropKey, PropValue};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::player::PlayerCore;

/// client 上报的终端 UI 状态(`Request::TerminalState` 写入、断开清除;
/// 每 tick 采样灌 `terminal` 复合属性)。多 client 后写赢。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalReport {
    /// 终端行数。
    pub(crate) rows: u16,

    /// 终端列数。
    pub(crate) cols: u16,

    /// 是否处于全屏播放态。
    pub(crate) fullscreen: bool,

    /// 终端窗口是否持有输入焦点。
    pub(crate) focused: bool,
}

/// 上次下发值的缓存:首轮全量产出(下游借此拿到初值),此后只发真变更。
#[derive(Default)]
pub(crate) struct PropsWatch {
    /// 属性 → 最近一次下发的值。
    last: Mutex<FxHashMap<PropKey, PropValue>>,
}

impl PropsWatch {
    /// 当前已下发属性值的快照(热重载播种新 VM 的属性缓存用)。
    fn snapshot(&self) -> Vec<(PropKey, PropValue)> {
        self.last
            .lock()
            .iter()
            .map(|(key, value)| (*key, value.clone()))
            .collect()
    }
}

impl PlayerCore {
    /// 属性树当前值快照(热重载起新 VM 前播种其缓存,经
    /// [`ScriptHost::seed_props`](mineral_script::ScriptHost::seed_props))。
    pub(crate) fn props_snapshot(&self) -> Vec<(PropKey, PropValue)> {
        self.inner.props.snapshot()
    }

    /// client 上报终端 UI 状态(serve 层处理 `Request::TerminalState`)。
    /// 下 tick `check_props` 自然 diff 下发,不走独立推送。
    ///
    /// # Return:
    ///   fullscreen **相对上一份已知态翻转**时给新值(供 fullscreen_changes 埋点);
    ///   无前态(client 刚连上,不算切换)或未变则 `None`。
    pub(crate) fn set_terminal_state(&self, report: TerminalReport) -> Option<bool> {
        let new_fullscreen = report.fullscreen;
        let mut slot = self.inner.ui_state.lock();
        let toggled = matches!(slot.as_ref(), Some(prev) if prev.fullscreen != new_fullscreen);
        *slot = Some(report);
        toggled.then_some(new_fullscreen)
    }

    /// client 断开时清空终端状态(`terminal` 属性回 `None`,脚本可感知离线)。
    pub(crate) fn clear_terminal_state(&self) {
        *self.inner.ui_state.lock() = None;
    }

    /// 采样全部属性、与上次值比较,变更逐项下发。background_loop 每 tick 调一次。
    ///
    /// 在播曲目真变更且新值是实曲时,顺带发脚本事件 `track_started`——
    /// 与 `player.song` 属性同源,远端起播 / 本地命中 / gapless 推进全覆盖
    /// (同曲重启 / 单曲循环不重复触发)。
    pub(crate) fn check_props(&self) {
        let snap = self.inner.audio.snapshot();
        let (current, song, mode, queue_len) = self.with_state(|st| {
            (
                st.current_song.clone(),
                st.current_song.as_ref().map(|s| s.id.qualified()),
                st.play_mode,
                st.queue.len(),
            )
        });
        let terminal = self.inner.ui_state.lock().map_or(PropValue::None, |t| {
            PropValue::Table(vec![
                ("rows".to_owned(), PropValue::Int(i64::from(t.rows))),
                ("cols".to_owned(), PropValue::Int(i64::from(t.cols))),
                ("fullscreen".to_owned(), PropValue::Bool(t.fullscreen)),
                ("focused".to_owned(), PropValue::Bool(t.focused)),
            ])
        });
        let state = if song.is_none() {
            "stopped"
        } else if snap.playing {
            "playing"
        } else {
            "paused"
        };
        let entries = [
            (
                PropKey::PlayerSong,
                song.map_or(PropValue::None, PropValue::Str),
            ),
            (PropKey::PlayerState, PropValue::Str(state.to_owned())),
            (
                PropKey::PlayerVolume,
                PropValue::Int(i64::from(snap.volume_pct)),
            ),
            (
                PropKey::PlayerPosition,
                PropValue::Int(saturating_i64(snap.position_ms / 1000)),
            ),
            (
                PropKey::PlayerMode,
                PropValue::Str(mode.script_name().to_owned()),
            ),
            (
                PropKey::QueueLength,
                PropValue::Int(saturating_i64(queue_len.try_into().unwrap_or(u64::MAX))),
            ),
            (PropKey::Terminal, terminal),
        ];
        let mut started = None;
        let mut last = self.inner.props.last.lock();
        for (key, value) in entries {
            if last.get(&key) != Some(&value) {
                if key == PropKey::PlayerSong && current.is_some() {
                    started = current.clone();
                }
                self.inner.notify.property_changed(key, &value);
                last.insert(key, value);
            }
        }
        drop(last);
        if let Some(song) = started {
            self.inner.notify.track_started(&song);
        }
    }
}

/// `u64` → `i64` 饱和转换(属性值域远小于 i64,饱和只是形式上的兜底)。
fn saturating_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}
