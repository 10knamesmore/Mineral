use derive_getters::Getters;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use crate::{
    ids::SongId,
    refs::{AlbumRef, ArtistRef},
    source::SourceKind,
    url::MediaUrl,
};

/// 一首歌曲的核心元数据。
///
/// 构造走 [`Song::builder`](Song::builder)(`#[non_exhaustive]`:新增字段不破坏外部构造);
/// 读取走 getter。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct Song {
    /// 歌曲 ID(自带 namespace,在其来源内唯一)。
    pub id: SongId,

    /// 歌名。
    pub name: String,

    /// 别名(译名 / 副标题等替代显示名,如外文曲名的中文译名),拿不到给 `None`。
    #[builder(default)]
    pub alias: Option<String>,

    /// 关联艺人(主艺人在前)。
    #[builder(default)]
    pub artists: Vec<ArtistRef>,

    /// 所属专辑(单曲为 `None`)。
    #[builder(default)]
    pub album: Option<AlbumRef>,

    /// 专辑内曲序(1-based);`None` = 来源没给 / 未探出。展示层据此排序、编号,
    /// 缺失时回落自然序而非充 `0`。serde 容缺:旧快照落 `None`。
    #[builder(default)]
    #[serde(default)]
    pub track_no: Option<u32>,

    /// 时长(ms);`None` = **未知**(来源接口没给 / 本地文件未探)——与「真的 0 ms」区分开,
    /// 展示层据此画占位而非 `0:00`,预排窗口等下游据此显式回落而非静默吃 0。
    #[builder(default)]
    pub duration_ms: Option<u64>,

    /// 封面图。远端 channel 通常给 `Remote(http(s)://...)`,
    /// 本地源若有内嵌封面可以给 `Local(...)` 指向缓存出来的文件。
    #[builder(default)]
    pub cover_url: Option<MediaUrl>,

    /// 这首歌的"原始位置"——本地源就是音频文件路径(`Local`);
    /// 远端源若已下载到缓存可以填 `Local`,否则为 `None`,需走 `song_urls`。
    #[builder(default)]
    pub source_url: Option<MediaUrl>,

    /// 自由标签集(genre 并入此处,不设专用 genre 字段——本地 genre 本就多值自由文本)。
    /// 大小写 / 别名归一是消费侧的事。serde 容缺:旧快照落空 `Vec`。
    #[builder(default)]
    #[serde(default)]
    pub tags: Vec<String>,

    /// 来源侧标记「无可播资源」(下架 / 无版权 / 失效)。列表元数据口径,可能滞后于
    /// 取流实况;展示层据此降权提示,**不禁播**——播放时取流失败自有拦截脚本补救。
    /// serde 容缺:旧缓存快照没有本字段,反序列化落 `false`。
    #[builder(default)]
    #[serde(default)]
    pub unavailable: bool,
}

impl Song {
    /// 来源(source)——派生自 [`Song::id`] 的 namespace。
    #[inline]
    pub fn source(&self) -> SourceKind {
        self.id.namespace()
    }
}

#[cfg(test)]
mod tests {
    use super::Song;

    /// 旧缓存快照(无 `unavailable` / `track_no` / `tags` 字段)反序列化落各自默认,不炸缓存。
    #[test]
    fn old_snapshot_missing_optional_fields_defaults() -> color_eyre::Result<()> {
        let song = serde_json::from_value::<Song>(serde_json::json!({
            "id": { "namespace": "netease", "value": "186016" },
            "name": "晴天",
            "alias": null,
            "artists": [],
            "album": null,
            "duration_ms": 269_000,
            "cover_url": null,
            "source_url": null
        }))?;
        assert!(!song.unavailable, "unavailable 缺失应落 false");
        assert_eq!(song.track_no, None, "track_no 缺失应落 None");
        assert!(song.tags.is_empty(), "tags 缺失应落空 Vec");
        Ok(())
    }
}
