//! copy 段(挂在 `TuiConfig` 下):复制菜单的自定义模板。
//!
//! 模板的 `template` 字段是 Lua function,没法进 serde 落型——加载管线在落型前
//! 把它从表里摘走、存进 VM named registry(键 [`mineral_script::registry::COPY_TEMPLATE_FNS`]),这里只落
//! `key`/`label`/`context` 三个展示字段。模板在当前 TUI 的 VM 中执行,
//! 返回的文本由同一 TUI 写入剪贴板。

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

/// 复制模板
#[config_section]
pub struct CopyConfig {
    /// 自定义复制菜单项，数组整体替换
    templates: Vec<CopyTemplate>,
}

/// 自定义复制模板；函数报错或超时不复制
#[config_section]
#[lua_optional_by_serde]
#[lua_extra_field(
    "template",
    "fun(e: mineral.Song|mineral.Playlist|mineral.Album|mineral.Artist): string",
    "渲染函数,返回进剪贴板的文本;收哪种表由 context 决定"
)]
pub struct CopyTemplate {
    /// 菜单快捷字母；冲突时后者覆盖
    #[serde(default)]
    key: Option<char>,

    /// 菜单名称
    label: String,

    /// 模板适用对象
    #[serde(default)]
    context: CopyContext,
}

/// 模板适用对象
#[lua_enum]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyContext {
    /// 歌曲
    #[default]
    Song,

    /// 歌单，含已加载曲目
    Playlist,

    /// 专辑，含已加载曲目
    Album,

    /// 艺人，含代表曲
    Artist,
}
