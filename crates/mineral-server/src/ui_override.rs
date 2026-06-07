//! 脚本 UI 旋钮覆盖表(`mineral.ui.override`):daemon **零认知**。
//!
//! key 是 opaque 字符串(约定 = 配置路径),daemon 不解释 —— 只存表、
//! 转发 [`Event::UiOverride`]、新 client 握手时重放全表;key→旋钮的
//! 类型化映射在 client 边缘做。session 级:daemon 重启即清,不碰配置文件。

use mineral_protocol::BusValue;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::player::PlayerCore;

/// 覆盖表本体(挂在 [`Inner`](crate::player) 上)。
#[derive(Default)]
pub(crate) struct UiOverrides {
    /// 旋钮键 → 当前覆盖值。
    map: Mutex<FxHashMap<String, BusValue>>,
}

impl PlayerCore {
    /// 落一条脚本覆盖:更新表 + 转发给订阅 client。
    ///
    /// 同值重写 / 撤销不存在的 key 不发事件(脚本常在 observe 回调里
    /// 无脑重设,diff 掉无谓的下推)。
    ///
    /// # Params:
    ///   - `key`: 旋钮键(opaque,不解释)
    ///   - `value`: 覆盖值;`None` = 撤销
    pub(crate) fn apply_ui_override(&self, key: String, value: Option<BusValue>) {
        let changed = {
            let mut map = self.inner.ui_overrides.map.lock();
            match &value {
                Some(v) => map.insert(key.clone(), v.clone()).as_ref() != Some(v),
                None => map.remove(&key).is_some(),
            }
        };
        if !changed {
            return;
        }
        self.notify().ui_override(key, value);
    }

    /// 覆盖表快照(新 client 握手订阅 `UiOverride` 时逐条重放)。
    pub(crate) fn ui_overrides_snapshot(&self) -> Vec<(String, BusValue)> {
        self.inner
            .ui_overrides
            .map
            .lock()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}
