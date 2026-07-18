//! 搜索端点的响应结构。

use serde::Deserialize;

use super::song::{AlbumSong, Artist};

/// cloudsearch type=1（单曲）的响应：歌曲是 `ar`/`al`/`dt` 形态，复用 [`AlbumSong`]
/// （与专辑/歌单 detail 同款，封面 `al.picUrl` 齐全）。
#[derive(Debug, Deserialize)]
pub struct CloudSongsResult {
    /// 命中的歌曲列表。
    #[serde(default)]
    pub songs: Vec<AlbumSong>,
}

/// `/weapi/search/get` type=10（专辑）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchAlbumsResult {
    /// 命中的专辑列表。
    #[serde(default)]
    pub albums: Vec<SearchAlbum>,
}

/// `/weapi/v1/album/{id}`（专辑详情）的响应:顶层 `album` 元信息 + `songs` 曲目。
///
/// `album` 复用 [`SearchAlbum`]——详情端点的专辑对象与搜索结果同形(id/name/artists/
/// description/company/publishTime/size/picUrl),区别仅在搜索端点 description 常为空、
/// 详情端点给完整简介。
#[derive(Debug, Deserialize)]
pub struct AlbumDetailResult {
    /// 专辑元信息(含完整简介)。
    pub album: SearchAlbum,

    /// 专辑曲目(`ar`/`al`/`dt` 形态)。
    #[serde(default)]
    pub songs: Vec<AlbumSong>,
}

/// 搜索结果里出现的专辑（结构与 song 子模块下的 `Album` 不同：多了 description 等）。
#[derive(Debug, Deserialize)]
pub struct SearchAlbum {
    /// 专辑 ID。
    pub id: i64,

    /// 专辑名。
    #[serde(default)]
    pub name: String,

    /// 主 artist（部分响应只给这个；`artists` 是全列表，优先后者）。
    #[serde(default)]
    pub artist: Option<Artist>,

    /// 全部 artist（多人 album 用；缺失时退回 `artist`）。
    #[serde(default)]
    pub artists: Vec<Artist>,

    /// 描述。
    #[serde(default)]
    pub description: String,

    /// 发行方（唱片公司 / 厂牌），可为 null。
    #[serde(default)]
    pub company: Option<String>,

    /// 发行时间（毫秒时间戳）。
    #[serde(default, rename = "publishTime")]
    pub publish_time: i64,

    /// 曲目数。
    #[serde(default)]
    pub size: u64,

    /// 封面 URL。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,
}

/// `/weapi/search/get` type=100（artist）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchArtistsResult {
    /// 命中的 artist 列表。
    #[serde(default)]
    pub artists: Vec<SearchArtist>,
}

/// 搜索结果里出现的 artist。
#[derive(Debug, Deserialize)]
pub struct SearchArtist {
    /// artist ID。
    pub id: i64,

    /// artist 名。
    #[serde(default)]
    pub name: String,

    /// 头像 URL。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,

    /// 粉丝数。`/weapi/search/get` 带真实值;cloudsearch 端点此字段为 `null`,
    /// 故用 `Option` 容错(缺失 / null 都落 `None`)。
    #[serde(default, rename = "fansSize")]
    pub fans_size: Option<u64>,
}

/// `/weapi/search/get` type=1000（歌单）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchPlaylistsResult {
    /// 命中的歌单列表。
    #[serde(default)]
    pub playlists: Vec<SearchPlaylist>,
}

/// 搜索结果里出现的歌单。
#[derive(Debug, Deserialize)]
pub struct SearchPlaylist {
    /// 歌单 ID。
    pub id: i64,

    /// 歌单名。
    #[serde(default)]
    pub name: String,

    /// 歌单描述。
    #[serde(default)]
    pub description: Option<String>,

    /// 歌单封面 URL。
    #[serde(default, rename = "coverImgUrl")]
    pub cover_img_url: Option<String>,

    /// 歌单内曲目数。
    #[serde(default, rename = "trackCount")]
    pub track_count: u64,

    /// 播放量（缺失 / null → `None`）。
    #[serde(default, rename = "playCount")]
    pub play_count: Option<u64>,

    /// 收藏数（搜索端点用 `bookCount`；用户 / 详情端点叫 `subscribedCount`，同一概念；
    /// 缺失 / null → `None`）。
    #[serde(default, rename = "bookCount")]
    pub book_count: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::CloudSongsResult;
    use crate::wire::de::from_value;

    /// 正常解析歌曲列表(cloudsearch ar/al/dt 形态)。真实抓取样本(歌单 17880415607):
    /// 迷星叫 `tns=null` + `alia=["Mayoiuta"]`(别名藏 alia、tns 为显式 null);
    /// 碧天伴走 `tns=null` 且 `alia` 缺失。锁住 null 容忍 + alia 解析。
    #[test]
    fn parses_song_list() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "songs": [
                { "id": 1, "name": "迷星叫", "tns": null, "alia": ["Mayoiuta"],
                  "ar": [{ "id": 1, "name": "MyGO!!!!!" }],
                  "al": { "id": 1, "name": "迷跡波", "picUrl": "https://p1.music.126.net/x.jpg" },
                  "dt": 211_373 },
                { "id": 2, "name": "碧天伴走", "tns": null, "ar": [{ "id": 1, "name": "MyGO!!!!!" }],
                  "al": { "id": 1, "name": "迷跡波" }, "dt": 256_000 }
            ]
        });
        let r: CloudSongsResult = from_value(raw)?;
        mineral_test::assert_snap_debug!(
            "搜索歌曲列表(MyGO 迷星叫 tns=null+alia / 碧天伴走 tns=null 无 alia)解析结构",
            r
        );
        Ok(())
    }

    /// 缺 `songs` 字段(无命中)→ 空列表,不报错。
    #[test]
    fn missing_songs_field_is_empty() -> color_eyre::Result<()> {
        let r: CloudSongsResult = from_value(serde_json::json!({}))?;
        assert!(r.songs.is_empty());
        Ok(())
    }

    /// 正常解析 artist 列表(stype=100)。
    #[test]
    fn parses_artist_list() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "artists": [
                { "id": 11127, "name": "Beyond", "picUrl": "https://p1.music.126.net/x.jpg",
                  "fansSize": 176_371 },
                // fansSize 缺失(cloudsearch 端点为 null)→ 落 None。
                { "id": 12345, "name": "Beyond乐队" }
            ]
        });
        let r: super::SearchArtistsResult = from_value(raw)?;
        mineral_test::assert_snap_debug!(
            "搜索 artist 列表(Beyond 带 fansSize / Beyond乐队 缺字段)解析结构",
            r
        );
        Ok(())
    }

    /// playlist 搜索:playCount/bookCount 解析(present→Some / 缺失→None),校验 rename。
    #[test]
    fn parses_playlist_counts() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "playlists": [
                { "id": 1, "name": "p", "trackCount": 41, "playCount": 149_123, "bookCount": 1706 },
                // 计量字段全缺 → None。
                { "id": 2, "name": "q" }
            ]
        });
        let r: super::SearchPlaylistsResult = from_value(raw)?;
        let first = r
            .playlists
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有首项"))?;
        assert_eq!(
            (first.track_count, first.play_count, first.book_count),
            (41, Some(149_123), Some(1706))
        );
        let second = r
            .playlists
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("应有次项"))?;
        assert_eq!(
            (second.track_count, second.play_count, second.book_count),
            (0, None, None)
        );
        Ok(())
    }

    /// 列表里出现 null artist / null album 名 → 跳过 / 空串,整体不失败。
    #[test]
    fn tolerates_null_artist_and_album_name() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "songs": [{ "id": 1, "name": "n", "ar": [null],
                        "al": { "id": 1, "name": null }, "dt": 0 }]
        });
        let r: CloudSongsResult = from_value(raw)?;
        mineral_test::assert_snap_debug!("搜索列表容错:null artist 跳过 + null album 名 → 空串", r);
        Ok(())
    }
}
