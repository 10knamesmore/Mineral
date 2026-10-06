//! 写出 tui 用户模板、默认值参考和本宿主 LuaLS 定义。

use std::path::Path;

use mineral_config::{InitOutcome, Result, overwrite, write_if_absent};

/// CLI 已创建 config_dir 与 lua/meta 后调用;已有用户文件不覆盖。
/// 默认参考与元数据随程序覆盖,运行时只加载编译进程序的默认值。
pub fn init(config_dir: &Path) -> Result<Vec<InitOutcome>> {
    let meta = config_dir.join("lua/meta");
    Ok(vec![
        write_if_absent(&config_dir.join("tui.lua"), include_str!("lua/tui.lua"))?,
        overwrite(&config_dir.join("tui-default.lua"), super::loader::DEFAULT)?,
        overwrite(&meta.join("tui.lua"), include_str!("lua/meta/tui.lua"))?,
        overwrite(
            &meta.join("tui-config.lua"),
            &super::lua_stub::meta_config_lua(),
        )?,
    ])
}
