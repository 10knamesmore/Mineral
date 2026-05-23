//! 歌曲、艺术家、专辑相关的协议结构。

use serde::{Deserialize, Deserializer};

/// 把 `null` 收成空串。网易云对失效 / 下架歌曲会把 name 类字段(歌名 / 艺术家名 /
/// 专辑名)返回 `null`,裸 `String` 反序列化会炸掉整批(已实锤:歌单 5036089714 的
/// `[2].al.name` 为 null);`#[serde(default)]` 只兜底字段缺失、兜不住显式 `null`,
/// 故这里把 `null` 与缺失统一收成空串。
fn string_or_null<'de, D>(de: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(de)?.unwrap_or_default())
}

/// 反序列化 `Vec<T>`,跳过其中的 `null` 元素。网易云对失效 / 下架歌曲会在 `ar`
/// (艺术家)数组里塞 `null`(已实锤:歌单 5036089714 的「张洲」`ar` 为 `[null]`),
/// 裸 `Vec<Artist>` 会炸(`null` 不是 struct)。这里把 `null` 元素直接丢弃。
fn vec_skip_null<'de, D, T>(de: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Vec::<Option<T>>::deserialize(de)?
        .into_iter()
        .flatten()
        .collect())
}

/// 协议层艺术家结构（出现在搜索结果、歌曲详情等多个端点）。
#[derive(Debug, Clone, Deserialize)]
pub struct Artist {
    /// 网易云艺术家数字 ID。
    pub id: i64,

    /// 艺术家名。
    #[serde(default, deserialize_with = "string_or_null")]
    pub name: String,
}

/// 协议层专辑结构。
#[derive(Debug, Clone, Deserialize)]
pub struct Album {
    /// 网易云专辑数字 ID。
    pub id: i64,

    /// 专辑名。
    #[serde(default, deserialize_with = "string_or_null")]
    pub name: String,

    /// 封面 URL（部分端点会缺）。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,
}

/// 搜索结果里出现的歌曲（用 `artists`、`album`、`duration` 字段）。
#[derive(Debug, Clone, Deserialize)]
pub struct SearchSong {
    /// 歌曲 ID。
    pub id: i64,

    /// 歌曲名。
    #[serde(default, deserialize_with = "string_or_null")]
    pub name: String,

    /// 艺术家列表。
    #[serde(default, deserialize_with = "vec_skip_null")]
    pub artists: Vec<Artist>,

    /// 专辑信息。
    pub album: Album,

    /// 时长（毫秒）。
    #[serde(default)]
    pub duration: u64,
}

/// 专辑/歌单详情里出现的歌曲（用 `ar`、`al`、`dt` 字段）。
#[derive(Debug, Clone, Deserialize)]
pub struct AlbumSong {
    /// 歌曲 ID。
    pub id: i64,

    /// 歌曲名。
    #[serde(default, deserialize_with = "string_or_null")]
    pub name: String,

    /// 艺术家列表（网易云在专辑/歌单 detail 端点用 `ar` 字段名）。
    #[serde(default, deserialize_with = "vec_skip_null")]
    pub ar: Vec<Artist>,

    /// 专辑信息（同上，端点字段名为 `al`）。
    pub al: Album,

    /// 时长（毫秒，字段名为 `dt`）。
    #[serde(default)]
    pub dt: u64,
}

/// 播放 URL 端点的单首歌响应。
#[derive(Debug, Clone, Deserialize)]
pub struct SongUrl {
    /// 歌曲 ID。
    pub id: i64,

    /// 实际可用的播放 URL（试听片段或拒绝时为 `None`）。
    #[serde(default)]
    pub url: Option<String>,

    /// 实际比特率（bps）。
    #[serde(default)]
    pub br: u32,

    /// 文件字节数。
    #[serde(default)]
    pub size: u64,

    /// 文件格式（如 `mp3` / `flac`），网易返回的字段名是 `type`。
    #[serde(default, rename = "type")]
    pub format: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::AlbumSong;
    use crate::wire::de::from_value;

    #[test]
    fn dead_song_with_null_artist_and_album_name() -> color_eyre::Result<()> {
        // 真实数据:歌单 5036089714 第 3 首「张洲」——失效单曲,al.name 为 null
        // 且 ar 为 [null](整个艺术家元素是 null)。两处都得容忍,否则整批反序列化失败。
        let raw = serde_json::json!([{
            "id": 1,
            "name": "张洲",
            "ar": [null],
            "al": { "id": 0, "name": null, "picUrl": "http://p4.music.126.net/x.jpg" },
            "dt": 0
        }]);
        let songs: Vec<AlbumSong> = from_value(raw)?;
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].name, "张洲");
        assert!(songs[0].ar.is_empty(), "null 艺术家元素应被跳过");
        assert_eq!(songs[0].al.name, "");
        Ok(())
    }
}
