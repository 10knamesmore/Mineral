//! 与实体投影和共用脚本值同步分发的 LuaLS 定义;不声明宿主配置 schema。

/// 两个宿主共用的音乐实体、查询与脚本值的 LuaLS 定义。
pub const TYPES_META: &str = include_str!("lua/meta/types.lua");
