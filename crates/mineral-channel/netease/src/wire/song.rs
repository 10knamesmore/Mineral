//! 歌曲、艺术家、专辑相关的协议结构。

use serde::Deserialize;

use super::de::{null_or_vec_skip_null, string_or_null, vec_skip_null};

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

/// 单曲权限块。cloudsearch 内联在每首歌上(`privilege`);song/detail 与歌单 detail
/// 在响应根给平行数组(`privileges`),由 [`merge_privileges`] 按 id 对回。
/// 只解析判可播性所需字段。
#[derive(Debug, Clone, Deserialize)]
pub struct Privilege {
    /// 歌曲 ID(平行数组形态按它对回歌曲)。
    pub id: i64,

    /// 状态:`0` 正常;`< 0` = 本源无可播资源(实测下架灰歌为 `-200`)。
    #[serde(default)]
    pub st: i64,
}

/// 专辑/歌单详情里出现的歌曲（用 `ar`、`al`、`dt` 字段）。
#[derive(Debug, Clone, Deserialize)]
pub struct AlbumSong {
    /// 歌曲 ID。
    pub id: i64,

    /// 歌曲名。
    #[serde(default, deserialize_with = "string_or_null")]
    pub name: String,

    /// 译名（`tns`；外文曲名的中文翻译）。真实数据里常为**显式 `null`**(不是缺失、也不是
    /// `[]`),`#[serde(default)]` 兜不住 null,故走 [`null_or_vec_skip_null`](null → 空)。
    #[serde(default, deserialize_with = "null_or_vec_skip_null")]
    pub tns: Vec<String>,

    /// 别名（`alia`）。网易「别名列表」——副标题 / 罗马音读法 / 出处说明（如「TV动画《…》片头曲」），
    /// 客户端把第一项显示作副标题。多数曲有 `alia` 而无 `tns`,故它是别名的主来源。
    #[serde(default, deserialize_with = "null_or_vec_skip_null")]
    pub alia: Vec<String>,

    /// 艺术家列表（网易云在专辑/歌单 detail 端点用 `ar` 字段名）。
    #[serde(default, deserialize_with = "vec_skip_null")]
    pub ar: Vec<Artist>,

    /// 专辑信息（同上，端点字段名为 `al`）。
    pub al: Album,

    /// 时长（毫秒，字段名为 `dt`）。
    #[serde(default)]
    pub dt: u64,

    /// 权限块(cloudsearch 内联;detail/歌单端点缺,由 [`merge_privileges`] 补)。
    #[serde(default)]
    pub privilege: Option<Privilege>,
}

/// 把响应根的平行 `privileges` 按 id 对回歌曲;已有内联权限块的不覆盖。
///
/// # Params:
///   - `songs`: 待补权限块的歌曲列表
///   - `privileges`: 响应根的平行数组(缺失时传空,no-op)
pub fn merge_privileges(songs: &mut [AlbumSong], privileges: Vec<Privilege>) {
    let mut by_id = privileges
        .into_iter()
        .map(|p| (p.id, p))
        .collect::<rustc_hash::FxHashMap<i64, Privilege>>();
    for song in songs {
        if song.privilege.is_none() {
            song.privilege = by_id.remove(&song.id);
        }
    }
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

    /// 单曲级状态码:`200` 可播;非 200(实测灰歌为 `404`)= 这首在本源无可播资源,
    /// 此时 `url` 为 null。注意信封 code 恒 200,灰只体现在这里。缺失落 `None`
    /// (旧响应形态,可播性回退看 `url`)。
    #[serde(default)]
    pub code: Option<i64>,

    /// 试听片段信息;非空 = `url` 只是试听片段(VIP 曲未授权),**不算可播**。
    #[serde(default, rename = "freeTrialInfo")]
    pub free_trial_info: Option<FreeTrialInfo>,
}

/// 试听片段的起止秒(只用于「存在与否」判断,字段容缺)。
#[derive(Debug, Clone, Deserialize)]
pub struct FreeTrialInfo {
    /// 片段起点(秒)。
    #[serde(default)]
    pub start: Option<i64>,

    /// 片段终点(秒)。
    #[serde(default)]
    pub end: Option<i64>,
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
    use super::{AlbumSong, FirstListenInfo};
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

    /// 平行 `privileges` 按 id 对回;已有内联权限块的不覆盖;数组缺条目的歌保持 `None`。
    #[test]
    fn merge_privileges_matches_by_id_without_overwriting_inline() -> color_eyre::Result<()> {
        use super::{Privilege, merge_privileges};

        let mut songs: Vec<AlbumSong> = from_value(serde_json::json!([
            { "id": 1, "name": "a", "al": { "id": 10, "name": "x" } },
            { "id": 2, "name": "b", "al": { "id": 11, "name": "y" },
              "privilege": { "id": 2, "st": 0 } },
            { "id": 3, "name": "c", "al": { "id": 12, "name": "z" } }
        ]))?;
        let privileges: Vec<Privilege> = from_value(serde_json::json!([
            { "id": 1, "st": -200 },
            { "id": 2, "st": -200 }
        ]))?;
        merge_privileges(&mut songs, privileges);

        let st_of = |i: usize| {
            songs
                .get(i)
                .and_then(|s| s.privilege.as_ref())
                .map(|p| p.st)
        };
        assert_eq!(st_of(0), Some(-200), "平行数组按 id 对回");
        assert_eq!(st_of(1), Some(0), "内联权限块不被平行数组覆盖");
        assert_eq!(st_of(2), None, "数组缺条目的歌保持 None");
        Ok(())
    }
}
