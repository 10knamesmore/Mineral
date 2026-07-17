//! copy 段(挂在 `TuiConfig` 下):复制菜单的自定义模板。
//!
//! 模板的 `template` 字段是 Lua function,没法进 serde 落型——加载管线在落型前
//! 把它从表里摘走、存进 VM named registry(键 [`COPY_TEMPLATE_FNS`]),这里只落
//! `key`/`label`/`context` 三个展示字段。**两边靠数组下标对位**:client 渲染菜单
//! 项与 daemon 取函数执行用的是同一份 config 的同一次 eval 序。

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

/// 摘走的模板函数数组在 VM named registry 里的键(daemon 脚本运行时按下标取用)。
pub const COPY_TEMPLATE_FNS: &str = "mineral.copy_template_fns";

/// copy 配置。
#[config_section]
pub struct CopyConfig {
    /// 自定义复制模板,追加在复制菜单内置项之后(数组整体替换)。
    templates: Vec<CopyTemplate>,
}

/// 一个自定义复制模板(的展示侧;渲染函数本体留在 VM,见模块文档)。
/// template 在 daemon 脚本运行时执行(看门狗超时保护,超时/报错只 toast 不复制)。
#[config_section]
#[lua_optional_by_serde]
#[lua_extra_field(
    "template",
    "fun(e: mineral.Song|mineral.Playlist): string",
    "渲染函数,返回进剪贴板的文本;收哪种表由 context 决定"
)]
pub struct CopyTemplate {
    /// 菜单快捷字母(单字符)。省略 = 仅导航 + Enter 可达;与内置项同字母时
    /// 顶掉内置的快捷位;用户项之间后者胜。
    #[serde(default)]
    key: Option<char>,

    /// 菜单显示名。
    label: String,

    /// 模板作用的上下文(决定回调收到的表形态与菜单项出现的位置)。
    #[serde(default)]
    context: CopyContext,
}

/// 复制模板的作用上下文。
#[lua_enum]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyContext {
    /// 歌曲上(回调收 `mineral.Song`)。
    #[default]
    Song,

    /// 歌单上(回调收 `mineral.Playlist`,含已加载曲目 `songs`)。
    Playlist,
}
