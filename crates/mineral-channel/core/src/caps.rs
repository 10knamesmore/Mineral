//! channel 能力声明类型。

use derive_getters::Getters;
use mineral_model::SearchKind;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

/// channel 能力声明:UI 据此决定渲染哪些入口(搜索类型、歌单管理键)。
///
/// 只是展示提示,不代替运行时错误处理——实现仍可能在运行时返回
/// [`Error::NotSupported`](crate::Error::NotSupported),两者互为防线:
/// caps 管"画不画入口"(静态、零成本),`NotSupported` 管"调了怎么办"
/// (运行时兜底,插件 channel 谎报能力时的安全网)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct ChannelCaps {
    /// 可全库搜索的实体类型。**空 = 不支持全库搜索;顺序即推荐顺序**
    /// (UI 切源联动时,默认搜索类型取第一项)。
    searchable: Vec<SearchKind>,

    /// 是否支持歌单写操作(建/删歌单、加/删歌、改名/改描述)。
    playlist_edit: bool,
}

#[cfg(test)]
mod tests {
    use super::ChannelCaps;
    use mineral_model::SearchKind;

    #[test]
    fn builder_and_getters_roundtrip() {
        let caps = ChannelCaps::builder()
            .searchable(vec![SearchKind::Song, SearchKind::Playlist])
            .playlist_edit(true)
            .build();
        assert_eq!(
            caps.searchable().as_slice(),
            &[SearchKind::Song, SearchKind::Playlist]
        );
        assert!(*caps.playlist_edit());
    }

    #[test]
    fn serde_roundtrip() -> color_eyre::Result<()> {
        let caps = ChannelCaps::builder()
            .searchable(vec![SearchKind::Album])
            .playlist_edit(false)
            .build();
        let json = serde_json::to_string(&caps)?;
        let back = serde_json::from_str::<ChannelCaps>(&json)?;
        assert_eq!(caps, back);
        Ok(())
    }

    #[test]
    fn empty_searchable_means_unsearchable() {
        let caps = ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .build();
        assert!(caps.searchable().is_empty());
    }
}
