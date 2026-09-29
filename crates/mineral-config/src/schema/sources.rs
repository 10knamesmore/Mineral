//! 音乐源段。
//!
//! 各源一个子段;`proxy` 用自定义反序列化表达「`false` = 禁用 / 字符串 = 代理 URL」,
//! 不用 `#[serde(untagged)]`(避免其错误路径含糊)。

use mineral_config_macros::{config_section, source_section};
use std::{num::NonZeroUsize, path::PathBuf};

use crate::schema::theme::ColorRef;

/// 摘走的 per-source `curate_playlists` 函数表在 VM named registry 里的键
/// (表键 = source 名,daemon 脚本运行时按源名取用)。
pub const CURATE_PLAYLISTS_SOURCE_FNS: &str = "mineral.curate_playlists_source_fns";

/// 摘走的跨源 `curate_playlists`(`sources` 表上的函数,合并列表 transform)
/// 在 VM named registry 里的键;未声明时为 Nil。
pub const CURATE_PLAYLISTS_MERGED_FN: &str = "mineral.curate_playlists_merged_fn";

/// 音乐来源
#[config_section]
#[lua_extra_field(
    "curate_playlists?",
    "mineral.CuratePlaylistsFn",
    "跨源策展:各源函数跑完、按注册序合并后的列表(条目带 source 字段),可全局排序/交错"
)]
pub struct SourcesConfig {
    /// 网易云音乐
    netease: NeteaseSection,

    /// 哔哩哔哩
    bilibili: BilibiliSection,

    /// 跨源收藏
    mineral: MineralSection,

    /// 本地音乐
    local: LocalSection,
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
            ("local", self.local.color()),
        ]
    }
}

/// 跨源收藏
#[config_section]
#[lua_extra_field(
    "curate_playlists?",
    "mineral.CuratePlaylistsFn",
    "该源歌单列表的呈现策展(过滤/改名/重排)"
)]
pub struct MineralSection {
    /// 来源徽标色
    color: ColorRef,

    /// 收藏曲目信息补全
    backfill: BackfillSection,
}

/// 收藏曲目信息补全
#[config_section]
pub struct BackfillSection {
    /// 每批补全歌曲数，不是请求数
    chunk_size: usize,

    /// 歌曲详情调用并发数
    max_concurrent: usize,
}

/// 哔哩哔哩
#[source_section]
#[lua_extra_field(
    "curate_playlists?",
    "mineral.CuratePlaylistsFn",
    "该源歌单(= 收藏夹)列表的呈现策展(过滤/改名/重排)"
)]
pub struct BilibiliSection {}

/// 网易云音乐
#[source_section]
#[lua_extra_field(
    "curate_playlists?",
    "mineral.CuratePlaylistsFn",
    "该源歌单列表的呈现策展(过滤/改名/重排)"
)]
pub struct NeteaseSection {
    /// 歌单曲目加载
    playlist_fetch: PlaylistFetchSection,
}

/// 歌单曲目加载
#[config_section]
pub struct PlaylistFetchSection {
    /// 每批歌曲数，须大于 0
    #[lua_type("integer")]
    batch_size: NonZeroUsize,

    /// 同一歌单请求并发数，须大于 0
    #[lua_type("integer")]
    max_concurrent: NonZeroUsize,
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

    /// 两个批次参数必须为正数，避免空批次或永远无法开始的请求。
    #[test]
    fn playlist_fetch_rejects_zero_limits() {
        for value in [
            serde_json::json!({"batch_size": 0, "max_concurrent": 3}),
            serde_json::json!({"batch_size": 500, "max_concurrent": 0}),
        ] {
            assert!(serde_json::from_value::<super::PlaylistFetchSection>(value).is_err());
        }
    }

    #[test]
    fn proxy_false_is_none() -> color_eyre::Result<()> {
        let s: NeteaseSection = serde_json::from_value(serde_json::json!({
            "playlist_fetch": {"batch_size": 500, "max_concurrent": 3},
            "timeout_secs": 100_u64, "proxy": false, "max_connections": 0_u64, "color": "red",
        }))?;
        assert_eq!(*s.proxy(), None);
        Ok(())
    }

    #[test]
    fn proxy_string_is_some() -> color_eyre::Result<()> {
        let s: NeteaseSection = serde_json::from_value(serde_json::json!({
            "playlist_fetch": {"batch_size": 500, "max_concurrent": 3},
            "timeout_secs": 100_u64, "proxy": "socks5://127.0.0.1:1080", "max_connections": 0_u64, "color": "red",
        }))?;
        assert_eq!(s.proxy().as_deref(), Some("socks5://127.0.0.1:1080"));
        Ok(())
    }

    #[test]
    fn proxy_true_errors() {
        assert!(
            serde_json::from_value::<NeteaseSection>(serde_json::json!({
                "playlist_fetch": {"batch_size": 500, "max_concurrent": 3},
            "timeout_secs": 100_u64, "proxy": true, "max_connections": 0_u64, "color": "red",
            }))
            .is_err(),
            "proxy = true 应报错"
        );
    }
}

/// 本地音乐，按直接含歌的目录建歌单
#[config_section]
#[lua_extra_field(
    "curate_playlists?",
    "mineral.CuratePlaylistsFn",
    "本地歌单列表的呈现策展"
)]
pub struct LocalSection {
    /// 扫描目录，接受绝对路径和 ~/，按 daemon 所在机器解析
    roots: Vec<PathBuf>,

    /// 来源徽标色
    color: ColorRef,
}
