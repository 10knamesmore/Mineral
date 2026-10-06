//! TUI 交互手感段(挂在 `TuiConfig` 下,经 `cfg.behavior()` 取)。
//!
//! const 审计补录的交互旋钮:音量/seek 步长、列表大步跳行、滚动步长与边距、
//! Library 过滤起播范围、自拉起 daemon 的退出续命。
//! 命令名 + 这些步长参数组装成可执行动作是 TUI 接线的事;本段只承载强类型值。

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

/// 交互行为
#[config_section]
pub struct BehaviorConfig {
    /// 音量调节步长（百分点）
    volume_step: u8,

    /// 快进快退步长
    seek_step_secs: u32,

    /// 大步快进快退步长
    seek_big_step_secs: u32,

    /// 光标大步跳行数
    list_jump_rows: u16,

    /// 光标距视口边缘的行数
    scrolloff: u16,

    /// 单次逐行滚动行数
    line_scroll_rows: usize,

    /// 单次翻页行数
    page_scroll_rows: usize,

    /// 距列表末尾多少行预取
    search_prefetch_rows: u16,

    /// 退出时关闭本次拉起的 daemon
    kill_spawned_daemon_on_exit: bool,

    /// 歌单光标记忆；搜索命中定位优先
    remember_track_pos: TrackPosMemory,

    /// 过滤后起播的队列范围
    filter_play_scope: FilterPlayScope,
}

/// 过滤后起播的队列范围
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum FilterPlayScope {
    /// 整个列表
    Collection,

    /// 仅匹配项
    Matches,
}

impl FilterPlayScope {
    /// 是否只用当前 filtered projection 建立播放队列。
    pub fn matches_only(self) -> bool {
        matches!(self, Self::Matches)
    }
}

/// 歌单光标记忆方式
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TrackPosMemory {
    /// 不记忆
    Off,

    /// 仅本次运行
    Session,

    /// 跨重启保留
    Persist,
}

impl TrackPosMemory {
    /// 是否启用记忆(`Session` / `Persist`)。
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// 是否要落盘(仅 `Persist`)。
    pub fn persists(self) -> bool {
        matches!(self, Self::Persist)
    }
}
