//! 音乐源段。
//!
//! 各源一个子段;`proxy` 用自定义反序列化表达「`false` = 禁用 / 字符串 = 代理 URL」,
//! 不用 `#[serde(untagged)]`(避免其错误路径含糊)。

use serde::Deserialize;

use crate::schema::theme::ColorRef;

/// 音乐源段聚合。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SourcesConfig {
    /// 网易云源段。
    netease: NeteaseSection,

    /// 哔哩哔哩源段。
    bilibili: BilibiliSection,
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
        ]
    }
}

/// 哔哩哔哩源段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。B站取流 URL(baseUrl)与 API 请求都要带
/// `Referer`(见 header 通道),超时 / 代理 / 并发在此配。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BilibiliSection {
    /// 请求超时(秒)。
    timeout_secs: u64,

    /// 代理:`None`(Lua `false`)= 禁用;`Some(url)` = 代理地址。
    #[serde(deserialize_with = "de_proxy")]
    proxy: Option<String>,

    /// 最大并发连接数(`0` = 不限)。
    max_connections: usize,

    /// 来源徽标色:token 名(随主题联动)或 `#rrggbb`(固定品牌色)。
    color: ColorRef,
}

/// 网易云源段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct NeteaseSection {
    /// 请求超时(秒)。
    timeout_secs: u64,

    /// 代理:`None`(Lua `false`)= 禁用;`Some(url)` = 代理地址。
    #[serde(deserialize_with = "de_proxy")]
    proxy: Option<String>,

    /// 最大并发连接数(`0` = 不限)。
    max_connections: usize,

    /// 来源徽标色:token 名(随主题联动)或 `#rrggbb`(固定品牌色)。
    color: ColorRef,
}

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
