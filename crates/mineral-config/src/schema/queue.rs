//! queue 段(顶层):脚本注册的具名队列变换。
//!
//! `transform` 字段是 Lua function,没法进 serde 落型——加载管线在落型前把它从表里
//! 摘走、存进 VM named registry(键 [`QUEUE_TRANSFORM_FNS`]),这里只落 `key`/`label`
//! 两个展示字段。**两边靠数组下标对位**:client 渲染菜单项与 daemon 取函数执行用的是
//! 同一份 config 的同一次 eval 序。
//!
//! 变换收到有序队列与一份位置上下文,返回新的有序队列。返回的每首歌必须在原队列里出现
//! 过(按 id 认;次数不限,复制已在队列的歌是允许的),不能凭空引入新歌。

use mineral_config_macros::config_section;

/// 摘走的变换函数数组在 VM named registry 里的键(daemon 脚本运行时按下标取用)。
pub const QUEUE_TRANSFORM_FNS: &str = "mineral.queue_transform_fns";

/// queue 配置。
#[config_section]
pub struct QueueConfig {
    /// 具名队列变换,出现在队列操作菜单的脚本段(数组整体替换)。
    transforms: Vec<QueueTransform>,
}

/// 一个具名队列变换(的展示侧;变换函数本体留在 VM,见模块文档)。
/// 函数在 daemon 脚本运行时执行(看门狗超时保护,超时/报错只 toast 不改队列)。
#[config_section]
#[lua_optional_by_serde]
#[lua_extra_field(
    "transform",
    "fun(queue: mineral.Song[], ctx: mineral.QueueCtx): mineral.Song[]",
    "变换函数,收有序队列与位置上下文,返回新的有序队列"
)]
pub struct QueueTransform {
    /// 菜单快捷字母(单字符)。省略 = 仅导航 + 激活可达;用户项之间后者胜。
    #[serde(default)]
    key: Option<char>,

    /// 菜单显示名。
    label: String,
}
