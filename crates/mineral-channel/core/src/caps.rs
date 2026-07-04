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
    use super::{ChannelCaps, render_web_url};
    use mineral_model::SearchKind;

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
