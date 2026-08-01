use derive_getters::Getters;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use crate::{CollectionIndex, Song};

/// Song 在某个 Playlist canonical snapshot 中的 membership relation。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct PlaylistEntry {
    /// Snapshot-relative 0-based absolute coordinate。
    pub index: CollectionIndex,

    /// 该 relation 指向的 Song。
    pub song: Song,
}

impl PlaylistEntry {
    /// 按输入 Song projection 的顺序构造连续的 0-based Playlist relation。
    ///
    /// 只应在 channel 已形成完整 canonical projection 的 adapter boundary 使用；partial
    /// hydration 若需要保留 gap，应显式携带原 index 构造 relation。
    ///
    /// # Params:
    ///   - `songs`: 已按 canonical playlist order 排列的 Song
    ///
    /// # Return:
    ///   带连续 0-based index 的 PlaylistEntry 列表。
    pub fn enumerate(songs: Vec<Song>) -> Vec<Self> {
        songs
            .into_iter()
            .zip(0_u64..)
            .map(|(song, index)| Self {
                index: CollectionIndex::new(index),
                song,
            })
            .collect()
    }
}
