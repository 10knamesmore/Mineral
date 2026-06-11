//! 歌曲、艺术家、专辑相关的协议结构。

use serde::Deserialize;

use super::de::{string_or_null, vec_skip_null};

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

/// 回忆坐标端点（`/api/content/activity/music/first/listen/info`）响应。
///
/// 只取 `data.musicTotalPlayDto.playCount` = 当前用户对该曲的真实累计播放次数;
/// 未登录 / 无记录时 `data` 可能缺失,故各级都用 `Option` 容忍。
#[derive(Debug, Clone, Deserialize)]
pub struct FirstListenInfo {
    /// 业务数据块（未登录 / 无记录时可能整块缺失）。
    #[serde(default)]
    pub data: Option<FirstListenData>,
}

/// 回忆坐标 `data` 块,本结构只关心累计播放统计。
#[derive(Debug, Clone, Deserialize)]
pub struct FirstListenData {
    /// 累计播放统计（字段名 `musicTotalPlayDto`，可能缺）。
    #[serde(default, rename = "musicTotalPlayDto")]
    pub music_total_play: Option<MusicTotalPlay>,
}

/// 累计播放统计块,本结构只取 `playCount`。
#[derive(Debug, Clone, Deserialize)]
pub struct MusicTotalPlay {
    /// 累计播放次数（字段名 `playCount`）。
    #[serde(default, rename = "playCount")]
    pub play_count: u32,
}

#[cfg(test)]
mod tests {
    use super::{AlbumSong, FirstListenInfo, SearchSong};
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
        mineral_test::assert_snap_debug!(
            "失效单曲:ar:[null] + al.name:null 的清洗结果(ar 应空、al.name 应空串)",
            songs
        );
        Ok(())
    }

    /// SearchSong 正常解析:artists / album / duration 各字段到位。
    #[test]
    fn search_song_parses_artists_album_duration() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "id": 42,
            "name": "壱雫空",
            "artists": [{ "id": 1, "name": "MyGO!!!!!" }],
            "album": { "id": 7, "name": "迷跡波", "picUrl": "http://p/x.jpg" },
            "duration": 264_000
        });
        let s: SearchSong = from_value(raw)?;
        mineral_test::assert_snap_debug!("SearchSong 全字段解析(MyGO 壱雫空 / 迷跡波)", s);
        Ok(())
    }

    /// AlbumSong 正常解析:ar / al / dt(detail 端点字段名)各到位。
    #[test]
    fn album_song_parses_ar_al_dt() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "id": 7,
            "name": "詩超絆",
            "ar": [{ "id": 1, "name": "MyGO!!!!!" }],
            "al": { "id": 3, "name": "迷跡波" },
            "dt": 233_000
        });
        let s: AlbumSong = from_value(raw)?;
        mineral_test::assert_snap_debug!(
            "AlbumSong detail 端点(ar/al/dt 字段名)解析(MyGO 詩超絆 / 迷跡波)",
            s
        );
        Ok(())
    }

    /// 回忆坐标:正常返回时取出 `musicTotalPlayDto.playCount`。
    #[test]
    fn first_listen_info_parses_play_count() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "code": 200,
            "data": {
                "musicTotalPlayDto": { "playCount": 18, "duration": 42 }
            }
        });
        let info: FirstListenInfo = from_value(raw)?;
        mineral_test::assert_snap_debug!("回忆坐标:data.musicTotalPlayDto.playCount=18 解析", info);
        Ok(())
    }

    /// 回忆坐标:未登录 / 无记录时 `data` 缺失,应得 `None` 而非反序列化失败。
    #[test]
    fn first_listen_info_tolerates_missing_data() -> color_eyre::Result<()> {
        let raw = serde_json::json!({ "code": 200 });
        let info: FirstListenInfo = from_value(raw)?;
        assert!(info.data.is_none(), "data 缺失应解析成 None");
        Ok(())
    }
}
