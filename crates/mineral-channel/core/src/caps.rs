//! channel 能力声明类型。

use derive_getters::Getters;
use mineral_model::SearchKind;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

/// artist 详情可含的分区种类。**穷尽枚举**(刻意非 `#[non_exhaustive]`):新增分区种类时,
/// 编译器会点出所有需要同步处理的 match。各源在
/// [`ArtistSections`] 里按需列出自己有哪些区。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistSectionKind {
    /// 热门曲(整源热门单曲)。音乐源(网易云等)有;视频源(B站)一个视频即一张专辑、无此概念。
    TopSongs,

    /// 专辑。
    Albums,
}

/// 某源 artist 详情包含哪些分区:**由源显式列出**(无默认——每个源都要表态,免得默认继承出错误
/// 的分区),**声明顺序 = 展示 / 切换顺序**,空 = 无分区面板。加新区种类只动 [`ArtistSectionKind`],
/// 不改此结构。UI 据此决定画哪些 tab、默认落哪区:多区可切,单区只画那一区、不给切换。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ArtistSections {
    /// 声明的分区,按展示顺序。
    kinds: Vec<ArtistSectionKind>,
}

impl ArtistSections {
    /// 按声明顺序构造(顺序即 UI tab / 切换顺序)。
    ///
    /// # Params:
    ///   - `kinds`: 该源含的分区,按展示顺序
    ///
    /// # Return:
    ///   分区声明。
    pub fn new(kinds: Vec<ArtistSectionKind>) -> Self {
        Self { kinds }
    }

    /// 声明的分区(只读切片,展示顺序)。
    pub fn kinds(&self) -> &[ArtistSectionKind] {
        &self.kinds
    }
}

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

    /// artist 详情的分区能力(见 [`ArtistSections`];每个源显式声明,无默认)。
    artist_sections: ArtistSections,
}
