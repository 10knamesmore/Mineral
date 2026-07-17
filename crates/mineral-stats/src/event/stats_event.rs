//! 顶层事件类型:把两个域的事件合成一个供 writer 消费的封装。

use crate::event::behavior::BehaviorEvent;
use crate::event::system::SystemEvent;
use crate::vocab::Actor;

/// 一条待落库的埋点事件。
///
/// 两个域的分野是**有无 actor**:行为域由人 / 脚本发起、统一带 actor;系统域是 daemon
/// 自治链路的副作用、无 actor。`ts` / `session_id` 不在此携带,由 writer 落库时给定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatsEvent {
    /// 行为域事件(有人 / 脚本发起,带 actor)。
    Behavior {
        /// 发起方。
        actor: Actor,

        /// 具体行为事件。
        event: BehaviorEvent,
    },

    /// 系统域事件(daemon 自治链路,无 actor)。
    System(SystemEvent),
}

impl StatsEvent {
    /// 事件 kind 名(= 目标表名),供 params gating(`collects_event`)与 audit 对账。
    ///
    /// # Return:
    ///   对应事件表名的 `&'static str`。
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Behavior { event, .. } => event.table(),
            Self::System(event) => event.table(),
        }
    }

    /// 事件归属的来源 name,供 `exclude_sources` 在发送出口统一过滤——被排除源的搜索 /
    /// 取数 / 取链等也一并无痕,与 plays 的排除语义对齐。
    ///
    /// # Return:
    ///   来源 name;事件不归属任何来源(全局事件)为 `None`
    pub fn source_name(&self) -> Option<&str> {
        match self {
            Self::Behavior { event, .. } => event.source_name(),
            Self::System(event) => event.source_name(),
        }
    }
}
