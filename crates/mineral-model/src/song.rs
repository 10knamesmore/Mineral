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

    /// 译名(原名的翻译,如外文曲名的中文译名),拿不到给 `None`。
    #[builder(default)]
    pub translation: Option<String>,

    /// 关联艺人(主艺人在前)。
    #[builder(default)]
    pub artists: Vec<ArtistRef>,

    /// 所属专辑(单曲为 `None`)。
    #[builder(default)]
    pub album: Option<AlbumRef>,

    /// 时长(ms),拿不到给 0。
    #[builder(default)]
    pub duration_ms: u64,

    /// 封面图。远端 channel 通常给 `Remote(http(s)://...)`,
    /// 本地源若有内嵌封面可以给 `Local(...)` 指向缓存出来的文件。
    #[builder(default)]
    pub cover_url: Option<MediaUrl>,

    /// 这首歌的"原始位置"——本地源就是音频文件路径(`Local`);
    /// 远端源若已下载到缓存可以填 `Local`,否则为 `None`,需走 `song_urls`。
    #[builder(default)]
    pub source_url: Option<MediaUrl>,

    /// 来源侧标记「无可播资源」(下架 / 无版权 / 失效)。列表元数据口径,可能滞后于
    /// 取流实况;展示层据此降权提示,**不禁播**——播放时取流失败自有拦截脚本补救。
    /// serde 容缺:旧缓存快照没有本字段,反序列化落 `false`。
    #[builder(default)]
    #[serde(default)]
    pub unavailable: bool,
}

impl Song {
    /// 来源 channel——派生自 [`Song::id`] 的 namespace。
    #[inline]
    pub fn source(&self) -> SourceKind {
        self.id.namespace()
    }
}

#[cfg(test)]
mod tests {
    use super::Song;

    /// 旧缓存快照(无 `unavailable` 字段)反序列化落 `false`,不炸缓存。
    #[test]
    fn old_snapshot_without_unavailable_defaults_false() -> color_eyre::Result<()> {
        let song = serde_json::from_value::<Song>(serde_json::json!({
            "id": { "namespace": "netease", "value": "186016" },
            "name": "晴天",
            "translation": null,
            "artists": [],
            "album": null,
            "duration_ms": 269_000,
            "cover_url": null,
            "source_url": null
        }))?;
        assert!(!song.unavailable, "字段缺失应落 false");
        Ok(())
    }
}
