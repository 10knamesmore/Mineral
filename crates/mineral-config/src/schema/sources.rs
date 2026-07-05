//! 音乐源段。
//!
//! 各源一个子段;`proxy` 用自定义反序列化表达「`false` = 禁用 / 字符串 = 代理 URL」,
//! 不用 `#[serde(untagged)]`(避免其错误路径含糊)。

use mineral_config_macros::{config_section, source_section};

use crate::schema::theme::ColorRef;

/// 摘走的 per-source `curate_playlists` 函数表在 VM named registry 里的键
/// (表键 = source 名,daemon 脚本运行时按源名取用)。
pub const CURATE_PLAYLISTS_SOURCE_FNS: &str = "mineral.curate_playlists_source_fns";

/// 摘走的跨源 `curate_playlists`(`sources` 表上的函数,合并列表 transform)
/// 在 VM named registry 里的键;未声明时为 Nil。
pub const CURATE_PLAYLISTS_MERGED_FN: &str = "mineral.curate_playlists_merged_fn";

/// 音乐源段聚合。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct SourcesConfig {
    /// 网易云源段。
    netease: NeteaseSection,

    /// 哔哩哔哩源段。
    bilibili: BilibiliSection,

    /// Mineral 聚合源段(全源收藏投影)。
    mineral: MineralSection,
}

impl SourcesConfig {
    /// 各源的徽标色 `(name, color)`——TUI 据此按 source 名把徽标解析成具体色(命中的走配置色,
    /// 未列出的源走中立兜底)。新增 native 源在此追加一项。
    ///
    /// # Return:
    ///   `(source name, 徽标 color)` 列表。
    pub fn source_colors(&self) -> Vec<(&str, &ColorRef)> {
        vec![
            ("netease", self.netease.color()),
            ("bilibili", self.bilibili.color()),
            ("mineral", self.mineral.color()),
        ]
    }
}

/// Mineral 聚合源段(全源收藏投影,source = `mineral`)。
///
/// 非网络源:没有 timeout / proxy 等网络旋钮(故不走 `#[source_section]`),
/// 可配徽标色 + 后台补 meta 的节流参数。
#[config_section]
pub struct MineralSection {
    /// 来源徽标色:token 名(随主题联动)或 `"#rrggbb"`(固定色)。
    color: ColorRef,

    /// 后台补 meta 的节流参数(聚合面如何逐步补全 sync 导入的、缺 meta 的收藏)。
    backfill: BackfillSection,
}

/// 聚合收藏后台补 meta 的节流参数。
///
/// sync 导入的远端红心先只有 id、无 meta,聚合视图重建不出。后台任务逐源(source-neutral:
/// 按各歌 namespace 走各自 channel 的 `songs_detail`,**不假设它是批量还是逐个**——批量源一次
/// 调用一个请求,逐个源一次调用内部循环,那是 channel 的事)分块拉详情补 persist,渐进填满。
#[config_section]
pub struct BackfillSection {
    /// 每次 `songs_detail` 调用处理多少 id:聚合面刷新的粒度,也限住单次调用时长。
    /// **非「请求数」**——请求怎么发是 channel 内部的事(批量 / 逐个)。
    chunk_size: usize,

    /// 并行几个 `songs_detail` 调用(并发上限即节流强度)。无论单次调用内部是一个请求还是
    /// 多个,同时最多 `max_concurrent` 个在飞;越小越温柔。
    max_concurrent: usize,
}

/// 哔哩哔哩源段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。共用网络字段
/// (timeout / proxy / max_connections / color)由 `#[source_section]` 注入,
/// 源特有字段写在体内。B站取流 URL(baseUrl)与 API 请求都要带 `Referer`
/// (见 header 通道)。
#[source_section]
pub struct BilibiliSection {}

/// 网易云源段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。共用网络字段由
/// `#[source_section]` 注入,源特有字段写在体内。
#[source_section]
pub struct NeteaseSection {}

/// 反序列化代理设置:Lua `false` → `None`(禁用);字符串 → `Some(url)`。
/// `true` 等其他形态报错(经 `serde_path_to_error` 带路径)。
///
/// # Params:
///   - `deserializer`: 字段反序列化器
///
/// # Return:
///   `None` 表禁用,`Some(url)` 表代理地址
fn de_proxy<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserializer.deserialize_any(ProxyVisitor)
}

/// `proxy` 字段访问器:容忍布尔 `false` 与字符串两种形态。
struct ProxyVisitor;

impl serde::de::Visitor<'_> for ProxyVisitor {
    type Value = Option<String>;

    /// 期望形态描述(serde 错误信息用)。
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("代理 URL 字符串或 `false`")
    }

    /// 布尔形态:仅 `false`(禁用)合法;`true` 无意义。
    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value {
            Err(E::custom(
                "proxy 须为代理 URL 字符串或 `false`(禁用),不接受 `true`",
            ))
        } else {
            Ok(None)
        }
    }

    /// 字符串形态:代理 URL。
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Some(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::NeteaseSection;

    /// 默认配置含 mineral 聚合源段(唯一旋钮 color),`source_colors` 出其条目
    /// (TUI 徽标据此着色,缺了就退中立兜底色)。
    #[test]
    fn mineral_section_in_defaults() -> color_eyre::Result<()> {
        let cfg = crate::Config::defaults()?;
        assert!(
            cfg.sources()
                .source_colors()
                .iter()
                .any(|(name, _)| *name == "mineral"),
            "source_colors 应含 mineral 条目"
        );
        Ok(())
    }

    #[test]
    fn proxy_false_is_none() -> color_eyre::Result<()> {
        let s: NeteaseSection = serde_json::from_value(serde_json::json!({
            "timeout_secs": 100_u64, "proxy": false, "max_connections": 0_u64, "color": "red",
        }))?;
        assert_eq!(*s.proxy(), None);
        Ok(())
    }

    #[test]
    fn proxy_string_is_some() -> color_eyre::Result<()> {
        let s: NeteaseSection = serde_json::from_value(serde_json::json!({
            "timeout_secs": 100_u64, "proxy": "socks5://127.0.0.1:1080", "max_connections": 0_u64, "color": "red",
        }))?;
        assert_eq!(s.proxy().as_deref(), Some("socks5://127.0.0.1:1080"));
        Ok(())
    }

    #[test]
    fn proxy_true_errors() {
        assert!(
            serde_json::from_value::<NeteaseSection>(serde_json::json!({
                "timeout_secs": 100_u64, "proxy": true, "max_connections": 0_u64, "color": "red",
            }))
            .is_err(),
            "proxy = true 应报错"
        );
    }
}
