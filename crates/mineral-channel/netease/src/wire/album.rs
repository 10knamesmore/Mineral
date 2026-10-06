//! 当前账号收藏专辑列表的远端响应；收藏时间等来源字段不进入统一模型。

use serde::Deserialize;

use super::song::Artist;

/// `/weapi/album/sublist` 的一页账号收藏专辑。
#[derive(Debug, Deserialize)]
pub struct SavedAlbumsResult {
    /// 按远端收藏列表顺序排列的专辑。
    pub data: Vec<SavedAlbum>,

    /// 来源明确给出的翻页信号。
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

/// 收藏列表中的专辑元信息；曲目按需从专辑详情加载。
#[derive(Debug, Deserialize)]
pub struct SavedAlbum {
    /// 网易云专辑 ID。
    pub id: i64,

    /// 专辑名称。
    pub name: String,

    /// 全部关联艺人。
    pub artists: Vec<Artist>,

    /// 专辑封面。
    #[serde(rename = "picUrl")]
    pub pic_url: Option<String>,

    /// 列表可能不提供曲目数，缺失时保持未知。
    pub size: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::SavedAlbumsResult;
    use crate::{convert::saved_album_to_model, wire::de::from_value};
    use mineral_model::{ArtistId, SourceKind};

    /// 收藏列表保留艺人顺序，缺少 size 与确实空专辑不能合并成同一种状态。
    #[test]
    fn saved_album_response_preserves_artists_and_unknown_count() -> color_eyre::Result<()> {
        let result: SavedAlbumsResult = from_value(serde_json::json!({
            "data": [
                {"id": 9, "name": "record", "artists": [{"id": 2, "name": "a"}, {"id": 3, "name": "b"}], "picUrl": null},
                {"id": 10, "name": "empty", "artists": [], "size": 0}
            ],
            "hasMore": true
        }))?;
        assert!(result.has_more);
        let albums = result
            .data
            .into_iter()
            .map(saved_album_to_model)
            .collect::<Vec<_>>();
        assert_eq!(albums.first().and_then(|album| album.track_count), None);
        assert_eq!(albums.get(1).and_then(|album| album.track_count), Some(0));
        assert_eq!(
            albums.first().map(|album| album
                .artists
                .iter()
                .map(|artist| artist.id.clone())
                .collect::<Vec<_>>()),
            Some(vec![
                ArtistId::new(SourceKind::NETEASE, "2"),
                ArtistId::new(SourceKind::NETEASE, "3")
            ])
        );
        assert!(albums.iter().all(|album| album.tracks.is_empty()));
        Ok(())
    }
}
