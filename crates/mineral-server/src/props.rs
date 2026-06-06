//! 属性树 diff:background_loop 每 tick 采样六个可观测属性,变更经
//! [`Notifier`](crate::notify::Notifier) 双路下发(订阅 client + 脚本)。
//!
//! 「高频合并只回末值」的语义由 tick 采样天然给出:tick 间无论变多少次,
//! 下游只看到采样时刻的末值。position 只在整秒值变化时产。

use mineral_script::{PropKey, PropValue};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::player::PlayerCore;

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

    /// 采样六属性、与上次值比较,变更逐项下发。background_loop 每 tick 调一次。
    pub(crate) fn check_props(&self) {
        let snap = self.inner.audio.snapshot();
        let (song, mode, queue_len) = self.with_state(|st| {
            (
                st.current_song.as_ref().map(|s| s.id.qualified()),
                st.play_mode,
                st.queue.len(),
            )
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
        ];
        let mut last = self.inner.props.last.lock();
        for (key, value) in entries {
            if last.get(&key) != Some(&value) {
                self.inner.notify.property_changed(key, &value);
                last.insert(key, value);
            }
        }
    }
}

/// `u64` → `i64` 饱和转换(属性值域远小于 i64,饱和只是形式上的兜底)。
fn saturating_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}
