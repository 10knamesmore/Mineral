//! 配置加载器与脚本执行器共用的 VM 回调存放位置;不携带配置 schema。

/// daemon 根 setup;未声明时为 Nil。
pub const DAEMON_SETUP_FN: &str = "mineral.daemon_setup_fn";

/// TUI 根 setup;未声明时为 Nil。
pub const TUI_SETUP_FN: &str = "mineral.tui_setup_fn";

/// daemon 队列变换函数表;按唯一操作名称取用。
pub const QUEUE_TRANSFORM_FNS: &str = "mineral.queue_transform_fns";

/// TUI 复制模板函数数组;按菜单下标取用。
pub const COPY_TEMPLATE_FNS: &str = "mineral.copy_template_fns";

/// daemon 的各来源歌单策展函数表;按来源名取用。
pub const CURATE_PLAYLISTS_SOURCE_FNS: &str = "mineral.curate_playlists_source_fns";

/// daemon 的跨来源歌单策展函数;未声明时为 Nil。
pub const CURATE_PLAYLISTS_MERGED_FN: &str = "mineral.curate_playlists_merged_fn";
