//! TUI 交互手感段(挂在 `TuiConfig` 下,经 `cfg.tui().behavior()` 取)。
//!
//! const 审计补录的交互旋钮:音量/seek 步长、列表大步跳行、自拉起 daemon 的退出续命。
//! 命令名 + 这些步长参数组装成可执行动作是 client 接线的事;本段只承载强类型值。

use serde::Deserialize;

/// 交互手感旋钮集合。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BehaviorConfig {
    /// 单次音量增减步长(百分点)。
    volume_step: u8,

    /// 单次 seek 步长(秒)。
    seek_step_secs: u32,

    /// 大步 seek 步长(秒)。
    seek_big_step_secs: u32,

    /// 列表大步跳行的行数。
    list_jump_rows: u16,

    /// TUI 退出时是否杀掉自己拉起的 daemon;`false` = 自拉起的 daemon 续命。
    kill_spawned_daemon_on_exit: bool,
}
