//! 歌手详情 / 歌手专辑列表端点的响应结构。

use serde::Deserialize;

use super::de::string_or_null;
use super::song::{AlbumSong, Artist};

/// `/weapi/v1/artist/{id}` 的响应(顶层即此结构,非 `result` 包裹)。
#[derive(Debug, Deserialize)]
pub struct ArtistDetailResult {
    /// 歌手信息。
    pub artist: ArtistInfo,

    /// 热门曲目(ar/al/dt 字段风格,与专辑/歌单 detail 同构)。
    #[serde(default, rename = "hotSongs")]
    pub hot_songs: Vec<AlbumSong>,
}

/// 详情 / 专辑列表端点里的歌手信息。
#[derive(Debug, Deserialize)]
pub struct ArtistInfo {
    /// 歌手数字 ID。
    pub id: i64,

    /// 歌手名。
    #[serde(default, deserialize_with = "string_or_null")]
    pub name: String,

    /// 头像 URL。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,

    /// 简介(可能缺失或 null)。
    #[serde(default, rename = "briefDesc", deserialize_with = "string_or_null")]
    pub brief_desc: String,

    /// 粉丝数(部分端点不带,缺失给 0)。
    #[serde(default, rename = "fansCount")]
    pub fans_count: u64,
}

/// `/api/artist/albums/{id}` 的响应。
#[derive(Debug, Deserialize)]
pub struct ArtistAlbumsResult {
    /// 专辑列表(网易云字段名如此,不只"热门",分页翻完即全集)。
    #[serde(default, rename = "hotAlbums")]
    pub hot_albums: Vec<ArtistAlbum>,
}

/// 歌手专辑列表里的专辑项。
#[derive(Debug, Deserialize)]
pub struct ArtistAlbum {
    /// 专辑数字 ID。
    pub id: i64,

    /// 专辑名。
    #[serde(default, deserialize_with = "string_or_null")]
    pub name: String,

    /// 封面 URL。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,

    /// 发行时间(毫秒时间戳)。
    #[serde(default, rename = "publishTime")]
    pub publish_time: i64,

    /// 主艺术家。
    #[serde(default)]
    pub artist: Option<Artist>,
}

#[cfg(test)]
mod tests {
    use super::{ArtistAlbumsResult, ArtistDetailResult};
    use crate::wire::de::from_value;

    /// 歌手详情:artist 元信息 + hotSongs(ar/al/dt 风格)整体解析。
    #[test]
    fn parses_artist_detail_with_hot_songs() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "artist": {
                "id": 11127, "name": "Beyond", "picUrl": "https://p1.music.126.net/x.jpg",
                "briefDesc": "香港摇滚乐队", "fansCount": 8_900_000
            },
            "hotSongs": [
                { "id": 1, "name": "海阔天空", "ar": [{ "id": 11127, "name": "Beyond" }],
                  "al": { "id": 9, "name": "乐与怒" }, "dt": 323_000 }
            ]
        });
        let r: ArtistDetailResult = from_value(raw)?;
        mineral_test::assert_snap_debug!("歌手详情(Beyond + 热门曲海阔天空)解析结构", r);
        Ok(())
    }

    /// 详情容错:briefDesc null、无 fansCount、hotSongs 缺失 → 空串/0/空列表。
    #[test]
    fn artist_detail_tolerates_missing_fields() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "artist": { "id": 1, "name": "X", "briefDesc": null }
        });
        let r: ArtistDetailResult = from_value(raw)?;
        assert_eq!(r.artist.brief_desc, "");
        assert_eq!(r.artist.fans_count, 0);
        assert!(r.hot_songs.is_empty());
        Ok(())
    }

    /// 歌手专辑列表:hotAlbums 解析(含发行时间与主艺术家)。
    #[test]
    fn parses_artist_albums() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "hotAlbums": [
                { "id": 9, "name": "乐与怒", "picUrl": "https://p1.music.126.net/a.jpg",
                  "publishTime": 736_185_600_000_i64, "artist": { "id": 11127, "name": "Beyond" } },
                { "id": 8, "name": "继续革命", "publishTime": 715_000_000_000_i64 }
            ],
            "more": false
        });
        let r: ArtistAlbumsResult = from_value(raw)?;
        mineral_test::assert_snap_debug!("歌手专辑列表(乐与怒/继续革命)解析结构", r);
        Ok(())
    }
}
