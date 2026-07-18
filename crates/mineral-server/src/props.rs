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
/// 每 tick 采样灌 `terminal` 复合属性)。
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

/// 各连接的终端上报 + last-wins 序:多终端平等,无主终端概念——`terminal`
/// 属性读**最近更新**的那条;断开只清自己的,若恰是最近者则回落到其余里
/// 最近更新的一条;全部离线回 `None`。
#[derive(Default)]
pub(crate) struct TerminalStates {
    /// conn id → (更新序号, 上报状态);读取取序号最大者。
    reports: FxHashMap<u64, (u64, TerminalReport)>,

    /// 单调更新计数(last-wins 的裁决序)。
    seq: u64,
}

impl TerminalStates {
    /// 记录一条上报,返回该连接**自己的**上一份状态(fullscreen 翻转埋点用:
    /// 别的终端的全屏态不算本终端的切换)。
    pub(crate) fn set(&mut self, conn: u64, report: TerminalReport) -> Option<TerminalReport> {
        self.seq += 1;
        self.reports
            .insert(conn, (self.seq, report))
            .map(|(_seq, prev)| prev)
    }

    /// 移除某连接的上报(断开收尾)。
    pub(crate) fn remove(&mut self, conn: u64) {
        self.reports.remove(&conn);
    }

    /// 当前生效的终端状态(最近更新者;无在线上报为 `None`)。
    pub(crate) fn current(&self) -> Option<TerminalReport> {
        self.reports
            .values()
            .max_by_key(|(seq, _report)| *seq)
            .map(|(_seq, report)| *report)
    }
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
    /// # Params:
    ///   - `conn`: 上报连接 id(per-conn 归属,见 [`TerminalStates`])
    ///   - `report`: 上报状态
    ///
    /// # Return:
    ///   fullscreen **相对该连接自己的上一份态翻转**时给新值(供
    ///   fullscreen_changes 埋点);无前态(刚连上,不算切换)或未变则 `None`。
    pub(crate) fn set_terminal_state(&self, conn: u64, report: TerminalReport) -> Option<bool> {
        let new_fullscreen = report.fullscreen;
        let prev = self.inner.ui_state.lock().set(conn, report);
        let toggled = matches!(prev, Some(p) if p.fullscreen != new_fullscreen);
        toggled.then_some(new_fullscreen)
    }

    /// 某连接断开,清其终端上报(全部离线时 `terminal` 属性回 `None`,
    /// 脚本可感知离线)。
    pub(crate) fn clear_terminal_state(&self, conn: u64) {
        self.inner.ui_state.lock().remove(conn);
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
        let terminal = self
            .inner
            .ui_state
            .lock()
            .current()
            .map_or(PropValue::None, |t| {
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
