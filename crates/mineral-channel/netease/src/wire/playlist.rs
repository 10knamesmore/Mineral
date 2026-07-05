//! 歌单端点的响应结构。

use serde::Deserialize;

use super::de::{null_or_vec_skip_null, string_or_null};
use super::song::{AlbumSong, Privilege};

/// `/api/v6/playlist/detail` 的响应:顶层 `playlist` 含元信息 + 曲目顺序 + 曲目。
#[derive(Debug, Deserialize)]
pub struct PlaylistDetailResult {
    /// 歌单对象(元信息 + trackIds + tracks)。
    pub playlist: PlaylistInfo,

    /// 与 `playlist.tracks` 平行的权限数组(判可播性;`limit=0` 轻请求为空)。
    #[serde(default)]
    pub privileges: Vec<Privilege>,
}

/// 歌单详情 / 用户歌单列表里的 playlist 对象。
///
/// 同一形状两用:详情端点带 `trackIds`/`tracks`(`limit=0` 时 tracks 空);用户歌单列表
/// 端点只有元信息(`trackIds`/`tracks` 空)。
#[derive(Debug, Deserialize)]
pub struct PlaylistInfo {
    /// 歌单数字 ID。
    pub id: i64,

    /// 歌单名。
    #[serde(default, deserialize_with = "string_or_null")]
    pub name: String,

    /// 简介。
    #[serde(default, deserialize_with = "string_or_null")]
    pub description: String,

    /// 封面 URL。
    #[serde(default, rename = "coverImgUrl")]
    pub cover_img_url: Option<String>,

    /// 曲目总数。
    #[serde(default, rename = "trackCount")]
    pub track_count: u64,

    /// 播放次数(部分端点不带 → `None`）。
    #[serde(default, rename = "playCount")]
    pub play_count: Option<u64>,

    /// 收藏数(详情 / 用户端点字段名 `subscribedCount`,搜索端点是 `bookCount`)。
    #[serde(default, rename = "subscribedCount")]
    pub subscribed_count: Option<u64>,

    /// 曲目版本戳(`trackUpdateTime`,unix ms;缓存条件刷新用)。
    #[serde(default, rename = "trackUpdateTime")]
    pub track_update_time: i64,

    /// 全量曲目 id 顺序(最新顺序以远端为准;`limit=0` 也返回)。
    ///
    /// 用户歌单列表项把它返回显式 `null`,故走 null 容忍解析(否则整批解析炸)。
    #[serde(
        default,
        rename = "trackIds",
        deserialize_with = "null_or_vec_skip_null"
    )]
    pub track_ids: Vec<TrackId>,

    /// 曲目(详情端点全拉时为 `ar`/`al`/`dt` 形态;`limit=0` 与用户歌单列表项返回 `null`)。
    #[serde(default, deserialize_with = "null_or_vec_skip_null")]
    pub tracks: Vec<AlbumSong>,
}

/// `trackIds` 数组的一项(只取数字 id)。
#[derive(Debug, Deserialize)]
pub struct TrackId {
    /// 曲目数字 ID。
    pub id: i64,
}

/// `/weapi/user/playlist` 的响应。
#[derive(Debug, Deserialize)]
pub struct UserPlaylistsResult {
    /// 用户歌单列表(只有元信息)。
    #[serde(default)]
    pub playlist: Vec<PlaylistInfo>,
}

/// `/api/playlist/create`(建单)的响应:`playlist` 是新建歌单对象。
///
/// 该对象是 [`PlaylistInfo`] 的子集(id/name/description/coverImgUrl/trackCount;无
/// trackIds/tracks 大头),故直接复用——免一次"建完再拉列表"的往返。
#[derive(Debug, Deserialize)]
pub struct CreatePlaylistResult {
    /// 新建的歌单对象(元信息)。
    pub playlist: PlaylistInfo,
}

#[cfg(test)]
mod tests {
    use super::{CreatePlaylistResult, PlaylistDetailResult, UserPlaylistsResult};
    use crate::wire::de::from_value;

    /// 用户歌单列表项把 `tracks`/`trackIds` 返回显式 `null`——必须容忍,不能炸整批。
    /// (真账号 `/weapi/user/playlist` 的几百项 tracks/trackIds 全是 null;`#[serde(default)]`
    /// 只兜键缺失、兜不住显式 null,故 wire 层需 null 容忍解析。)
    #[test]
    fn user_playlists_tolerates_null_tracks_and_track_ids() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "playlist": [
                { "id": 1, "name": "我喜欢的音乐", "description": null,
                  "trackCount": 557, "tracks": null, "trackIds": null,
                  "subscribedCount": 0 }
            ]
        });
        let r: UserPlaylistsResult = from_value(raw)?;
        let p = r
            .playlist
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有一项"))?;
        assert_eq!(p.name, "我喜欢的音乐");
        assert_eq!(p.track_count, 557);
        assert!(p.tracks.is_empty(), "null tracks → 空");
        assert!(p.track_ids.is_empty(), "null trackIds → 空");
        Ok(())
    }

    /// 详情端点(全拉)的正常数组形态仍解析——null 容忍不能把正常路径搞坏。
    #[test]
    fn playlist_detail_parses_normal_arrays() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "playlist": {
                "id": 9, "name": "x", "trackUpdateTime": 100,
                "trackIds": [{ "id": 1 }, { "id": 2 }],
                "tracks": [
                    { "id": 1, "name": "a", "ar": [{ "id": 5, "name": "ar" }],
                      "al": { "id": 7, "name": "al" }, "dt": 1000 }
                ]
            }
        });
        let r: PlaylistDetailResult = from_value(raw)?;
        assert_eq!(r.playlist.track_ids.len(), 2);
        assert_eq!(r.playlist.tracks.len(), 1);
        Ok(())
    }

    /// 建单响应缺 `playlist` 对象 → 显式 serde missing-field 报错(而非 panic / 默认值)。
    #[test]
    fn create_result_rejects_missing_playlist() -> color_eyre::Result<()> {
        let err = from_value::<CreatePlaylistResult>(serde_json::json!({ "code": 200 }))
            .err()
            .ok_or_else(|| color_eyre::eyre::eyre!("应解析失败"))?;
        assert!(
            format!("{err}").contains("playlist"),
            "错误应点名缺失的 `playlist`,实得:{err}"
        );
        Ok(())
    }
}
