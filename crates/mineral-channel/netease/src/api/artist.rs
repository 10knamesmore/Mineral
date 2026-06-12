//! 歌手端点(详情 + 专辑列表)。

use mineral_model::{Album, Artist, ArtistId, ArtistRef, SourceKind};

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::convert::{album_song_to_model, parse_remote_opt};
use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::artist::{ArtistAlbum, ArtistAlbumsResult, ArtistDetailResult};

/// 歌手详情:简介元信息 + 热门曲目。
pub async fn artist_detail(transport: &Transport, id: &ArtistId) -> Result<Artist> {
    let path = format!("/weapi/v1/artist/{}", id.as_str());
    let raw = transport
        .request(RequestSpec {
            path: &path,
            crypto: Crypto::Weapi,
            params: serde_json::Map::new(),
            ua: UaKind::Any,
        })
        .await?;
    // 此端点响应在顶层平铺(artist / hotSongs 与 code 同级),无 result 包裹
    let parsed: ArtistDetailResult = crate::wire::de::from_value(raw)?;
    Ok(detail_to_model(parsed))
}

/// 歌手的专辑列表(分页;曲目留空,按需走 `songs_in_album`)。
pub async fn artist_albums(
    transport: &Transport,
    id: &ArtistId,
    offset: u32,
    limit: u32,
) -> Result<Vec<Album>> {
    let path = format!("/api/artist/albums/{}", id.as_str());
    let mut params = serde_json::Map::new();
    params.insert("limit".into(), serde_json::json!(limit.to_string()));
    params.insert("offset".into(), serde_json::json!(offset.to_string()));
    params.insert("total".into(), serde_json::json!("true"));
    let raw = transport
        .request(RequestSpec {
            path: &path,
            crypto: Crypto::Weapi,
            params,
            ua: UaKind::Any,
        })
        .await?;
    let parsed: ArtistAlbumsResult = crate::wire::de::from_value(raw)?;
    Ok(parsed.hot_albums.into_iter().map(album_to_model).collect())
}

/// 详情响应 → 统一 [`Artist`]。
fn detail_to_model(r: ArtistDetailResult) -> Artist {
    Artist {
        id: ArtistId::new(SourceKind::NETEASE, r.artist.id.to_string()),
        name: r.artist.name,
        description: r.artist.brief_desc,
        follower_count: r.artist.fans_count,
        avatar_url: parse_remote_opt(r.artist.pic_url.as_deref()),
        songs: r.hot_songs.into_iter().map(album_song_to_model).collect(),
    }
}

/// 专辑列表项 → 统一 [`Album`](曲目留空)。
fn album_to_model(a: ArtistAlbum) -> Album {
    Album {
        id: mineral_model::AlbumId::new(SourceKind::NETEASE, a.id.to_string()),
        name: a.name,
        artists: a
            .artist
            .map(|x| {
                vec![ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, x.id.to_string()),
                    name: x.name,
                }]
            })
            .unwrap_or_default(),
        description: String::new(),
        publish_time_ms: a.publish_time,
        cover_url: parse_remote_opt(a.pic_url.as_deref()),
        songs: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{album_to_model, detail_to_model};
    use crate::wire::de::from_value;

    /// 详情响应 → model:id 入 NETEASE namespace、briefDesc → description、
    /// hotSongs → songs 全量映射。
    #[test]
    fn detail_maps_to_model_artist() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "artist": { "id": 11127, "name": "Beyond", "briefDesc": "香港摇滚乐队",
                        "picUrl": "https://p1.music.126.net/x.jpg", "fansCount": 42 },
            "hotSongs": [
                { "id": 1, "name": "海阔天空", "ar": [{ "id": 11127, "name": "Beyond" }],
                  "al": { "id": 9, "name": "乐与怒" }, "dt": 323_000 }
            ]
        });
        let model = detail_to_model(from_value(raw)?);
        mineral_test::assert_snap_debug!("歌手详情映射成统一 Artist(Beyond + 1 热门曲)", model);
        Ok(())
    }

    /// 专辑列表项 → model:无主艺术家时 artists 为空,曲目恒空。
    #[test]
    fn album_item_without_artist_maps_to_empty_artists() -> color_eyre::Result<()> {
        let raw =
            serde_json::json!({ "id": 8, "name": "继续革命", "publishTime": 715_000_000_000_i64 });
        let model = album_to_model(from_value(raw)?);
        assert!(model.artists.is_empty());
        assert!(model.songs.is_empty());
        assert_eq!(model.publish_time_ms, 715_000_000_000);
        Ok(())
    }
}
