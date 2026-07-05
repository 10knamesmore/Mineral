//! channel 能力声明类型。

use derive_getters::Getters;
use mineral_model::SearchKind;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

/// artist 详情可含的分区种类。**穷尽枚举**(刻意非 `#[non_exhaustive]`):未来加新区(单曲 /
/// 合辑 / 出现于 等)时加一个变体,编译器即点出所有待处理的 match,不漏。各源在
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

    /// 歌曲网页(分享链接)模板。占位语义(渲染统一走 [`render_web_url`]):
    /// `{id}` 填**整段裸** id(如 `"https://music.163.com/song?id={id}"`);裸 id 是
    /// `:` 分段的复合值时可用 `{0}`/`{1}`… 位置占位取各段(如 B 站裸 id `bvid:page` 配
    /// `".../video/{0}?p={1}"`)。`None` = 该源没有网页形态(本地文件等),
    /// UI 不渲染「复制链接」类入口。
    #[builder(default)]
    song_web_url: Option<String>,

    /// 歌单网页(分享链接)模板,占位语义同 [`Self::song_web_url`]。
    #[builder(default)]
    playlist_web_url: Option<String>,
}

/// 按源声明的网页模板渲染分享链接(TUI 复制菜单与 Lua 投影共用,勿各自实现)。
///
/// 占位语义(与 [`ChannelCaps::song_web_url`] 文档一致):`{id}` 填整段裸 id;
/// `{0}`/`{1}`… 填裸 id 按 `:` 拆出的对应段(越界的占位原样保留,提示模板与
/// id 形状不符,不静默吞)。
///
/// # Params:
///   - `template`: caps 声明的模板
///   - `raw_id`: 裸 id(`Id::value()`)
///
/// # Return:
///   渲染后的网页链接。
pub fn render_web_url(template: &str, raw_id: &str) -> String {
    let mut out = template.replace("{id}", raw_id);
    for (i, seg) in raw_id.split(':').enumerate() {
        out = out.replace(&format!("{{{i}}}"), seg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{ArtistSectionKind, ArtistSections, ChannelCaps, render_web_url};
    use mineral_model::SearchKind;

    /// 两区皆有的 artist 分区(音乐源形态测试夹具)。
    fn both_sections() -> ArtistSections {
        ArtistSections::new(vec![ArtistSectionKind::TopSongs, ArtistSectionKind::Albums])
    }

    /// `{id}` 整段替换(单段 id 的既有语义不变)。
    #[test]
    fn render_whole_id_placeholder() {
        assert_eq!(
            render_web_url("https://x.example/song?id={id}", "12345"),
            "https://x.example/song?id=12345"
        );
    }

    /// `{0}`/`{1}` 位置占位:复合裸 id 按 `:` 拆段对位填入。
    #[test]
    fn render_positional_segments() {
        assert_eq!(
            render_web_url("https://www.bilibili.com/video/{0}?p={1}", "BV1xx:3"),
            "https://www.bilibili.com/video/BV1xx?p=3"
        );
    }

    /// 越界占位原样保留(模板要 {1} 而 id 只有一段):暴露形状不符,不静默吞段。
    #[test]
    fn render_out_of_range_placeholder_kept() {
        assert_eq!(
            render_web_url("https://x.example/{0}?p={1}", "solo"),
            "https://x.example/solo?p={1}"
        );
    }

    #[test]
    fn builder_and_getters_roundtrip() {
        let caps = ChannelCaps::builder()
            .searchable(vec![SearchKind::Song, SearchKind::Playlist])
            .playlist_edit(true)
            .artist_sections(both_sections())
            .build();
        assert_eq!(
            caps.searchable().as_slice(),
            &[SearchKind::Song, SearchKind::Playlist]
        );
        assert!(*caps.playlist_edit());
    }

    /// artist 分区逐一显式声明,顺序即展示序:音乐源热门曲 + 专辑,视频源(B站)只有专辑。
    #[test]
    fn artist_sections_declared_per_source() {
        assert_eq!(
            both_sections().kinds(),
            &[ArtistSectionKind::TopSongs, ArtistSectionKind::Albums],
            "音乐源两区,热门曲在前"
        );
        let video = ArtistSections::new(vec![ArtistSectionKind::Albums]);
        assert_eq!(
            video.kinds(),
            &[ArtistSectionKind::Albums],
            "视频源只有专辑区"
        );
    }

    #[test]
    fn serde_roundtrip() -> color_eyre::Result<()> {
        let caps = ChannelCaps::builder()
            .searchable(vec![SearchKind::Album])
            .playlist_edit(false)
            .artist_sections(both_sections())
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
            .artist_sections(both_sections())
            .build();
        assert!(caps.searchable().is_empty());
    }
}
