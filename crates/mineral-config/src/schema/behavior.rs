//! TUI 交互手感段(挂在 `TuiConfig` 下,经 `cfg.tui().behavior()` 取)。
//!
//! const 审计补录的交互旋钮:音量/seek 步长、列表大步跳行、滚动步长与边距、
//! 自拉起 daemon 的退出续命。
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

    /// 光标与列表视口上下边缘保持的最小行距(nvim `scrolloff`;`0` = 贴边才滚)。
    scrolloff: u16,

    /// 单行档滚动(`<C-d>` / `<C-u>`)一次移动的行数。列表与全屏歌词共用。
    line_scroll_rows: usize,

    /// 翻页档滚动(`<C-f>` / `<C-b>`)一次移动的行数。
    page_scroll_rows: usize,

    /// 搜索结果列懒分页预取触发半径:光标进入距已加载结果末行此行数内、且该 (源,kind) 桶
    /// 未榨干时,自动派发下一页搜索任务。越大越早预取(滚动越顺滑、请求越靠前)。
    search_prefetch_rows: u16,

    /// TUI 退出时是否杀掉自己拉起的 daemon;`false` = 自拉起的 daemon 续命。
    kill_spawned_daemon_on_exit: bool,

    /// 歌单内光标位置记忆档:退出曲目列表时记住位置,下次进入恢复。
    remember_track_pos: TrackPosMemory,
}

/// 歌单内光标位置记忆的生效档位。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TrackPosMemory {
    /// 不记不恢复,每次进歌单回到第 0 行。
    Off,

    /// 只在本次运行内记忆,关掉 TUI 即忘。
    Session,

    /// 记忆并落盘,跨重启保留。
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
