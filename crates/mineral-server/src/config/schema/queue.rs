//! daemon 中执行的具名队列变换;TUI 只收到可用操作名称。
//!
//! 函数在落型前摘入 VM registry,按唯一名称取用。返回的每首歌必须在原队列中
//! 出现过(按 id 认,允许重复现有歌曲),不能引入外来歌曲。

use mineral_config_macros::config_section;

/// 队列变换
#[config_section]
pub struct QueueConfig {
    /// 自定义队列变换，名称不可为空或重复;数组整体替换
    #[serde(deserialize_with = "de_transforms")]
    transforms: Vec<QueueTransform>,
}

/// 队列变换；函数报错或超时不改队列
#[config_section]
#[lua_optional_by_serde]
#[lua_extra_field(
    "transform",
    "fun(queue: mineral.Song[], ctx: mineral.QueueCtx): mineral.Song[]",
    "变换函数,收有序队列与位置上下文,返回新的有序队列"
)]
pub struct QueueTransform {
    /// 唯一的人类可读名称，同时作为操作标识与菜单名称
    name: String,
}

/// 校验操作名称,避免 registry 覆盖或 TUI 按名称请求到另一项操作。
fn de_transforms<'de, D>(deserializer: D) -> Result<Vec<QueueTransform>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    let transforms = Vec::<QueueTransform>::deserialize(deserializer)?;
    let mut names = rustc_hash::FxHashSet::<&str>::default();
    for transform in &transforms {
        let name = transform.name();
        if name.trim().is_empty() {
            return Err(serde::de::Error::custom("队列变换名称不可为空"));
        }
        if !names.insert(name.as_str()) {
            return Err(serde::de::Error::custom(format!(
                "队列变换名称重复: {name}"
            )));
        }
    }
    Ok(transforms)
}
