use serde::{Deserialize, Serialize};

use crate::{
    bitrate::BitRate, format::AudioFormat, ids::SongId, source::SourceKind, url::MediaUrl,
};

/// 一首歌的可播放 URL + 元信息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayUrl {
    /// 关联的歌曲 ID(自带 namespace)。
    pub song_id: SongId,
    /// 播放地址:远端 stream URL 用 `Remote`,本地文件用 `Local`。
    pub url: MediaUrl,
    /// 实际比特率(bps)。
    pub bitrate_bps: u32,
    /// channel 解析后的归一化音质等级。
    pub quality: BitRate,
    /// 文件大小(bytes),拿不到给 0。
    pub size: u64,
    /// 文件格式——channel 实际提供的容器格式(`mp3` / `flac` 等),拿不到为 `Other("")`。
    pub format: AudioFormat,
}

impl PlayUrl {
    /// 来源 channel——派生自 [`PlayUrl::song_id`] 的 namespace。
    #[inline]
    pub fn source(&self) -> SourceKind {
        self.song_id.namespace()
    }
}
