//! 搜索端点的响应结构。

use serde::Deserialize;

use super::song::{Artist, SearchSong};

/// `/weapi/search/get` type=1（单曲）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchSongsResult {
    /// 命中的歌曲列表。
    #[serde(default)]
    pub songs: Vec<SearchSong>,
}

/// `/weapi/search/get` type=10（专辑）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchAlbumsResult {
    /// 命中的专辑列表。
    #[serde(default)]
    pub albums: Vec<SearchAlbum>,
}

/// 搜索结果里出现的专辑（结构与 song 子模块下的 `Album` 不同：多了 description 等）。
#[derive(Debug, Deserialize)]
pub struct SearchAlbum {
    /// 专辑 ID。
    pub id: i64,

    /// 专辑名。
    #[serde(default)]
    pub name: String,

    /// 主艺术家。
    #[serde(default)]
    pub artist: Option<Artist>,

    /// 描述。
    #[serde(default)]
    pub description: String,

    /// 发行时间（毫秒时间戳）。
    #[serde(default, rename = "publishTime")]
    pub publish_time: i64,

    /// 封面 URL。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,
}

/// `/weapi/search/get` type=100（歌手）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchArtistsResult {
    /// 命中的歌手列表。
    #[serde(default)]
    pub artists: Vec<SearchArtist>,
}

/// 搜索结果里出现的歌手。
#[derive(Debug, Deserialize)]
pub struct SearchArtist {
    /// 歌手 ID。
    pub id: i64,

    /// 歌手名。
    #[serde(default)]
    pub name: String,

    /// 头像 URL。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,
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
}

#[cfg(test)]
mod tests {
    use super::SearchSongsResult;
    use crate::wire::de::from_value;

    /// 正常解析歌曲列表(多首)。
    #[test]
    fn parses_song_list() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "songs": [
                { "id": 1, "name": "迷星叫", "artists": [{ "id": 1, "name": "MyGO!!!!!" }],
                  "album": { "id": 1, "name": "迷跡波" }, "duration": 248_000 },
                { "id": 2, "name": "碧天伴走", "artists": [{ "id": 1, "name": "MyGO!!!!!" }],
                  "album": { "id": 1, "name": "迷跡波" }, "duration": 256_000 }
            ]
        });
        let r: SearchSongsResult = from_value(raw)?;
        mineral_test::assert_snap_debug!("搜索歌曲列表(MyGO 迷星叫 / 碧天伴走)解析结构", r);
        Ok(())
    }

    /// 缺 `songs` 字段(无命中)→ 空列表,不报错。
    #[test]
    fn missing_songs_field_is_empty() -> color_eyre::Result<()> {
        let r: SearchSongsResult = from_value(serde_json::json!({}))?;
        assert!(r.songs.is_empty());
        Ok(())
    }

    /// 正常解析歌手列表(stype=100)。
    #[test]
    fn parses_artist_list() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "artists": [
                { "id": 11127, "name": "Beyond", "picUrl": "https://p1.music.126.net/x.jpg" },
                { "id": 12345, "name": "Beyond乐队" }
            ]
        });
        let r: super::SearchArtistsResult = from_value(raw)?;
        mineral_test::assert_snap_debug!("搜索歌手列表(Beyond / Beyond乐队)解析结构", r);
        Ok(())
    }

    /// 列表里出现 null 艺术家 / null 专辑名 → 跳过 / 空串,整体不失败。
    #[test]
    fn tolerates_null_artist_and_album_name() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "songs": [{ "id": 1, "name": "n", "artists": [null],
                        "album": { "id": 1, "name": null }, "duration": 0 }]
        });
        let r: SearchSongsResult = from_value(raw)?;
        mineral_test::assert_snap_debug!("搜索列表容错:null 艺人跳过 + null 专辑名 → 空串", r);
        Ok(())
    }
}
