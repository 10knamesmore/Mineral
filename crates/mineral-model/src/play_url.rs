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
    /// 位深(bits per sample),如 16 / 24。仅本地无损文件经实测有值;流式来源的接口不返回
    /// 位深、有损格式(mp3/aac)亦无此概念,这些情形均为 `None`(显示侧据此省略位深段)。
    pub bit_depth: Option<u8>,

    /// 取流时必须附加的请求头(如 B站 baseUrl 播放需 `Referer`);空 = 无附加头。键值对而非
    /// map:保序、允许重复头。随 `PlayUrl` 一起经 IPC 序列化,不能在播放/下载链路丢失。
    pub stream_headers: Vec<(String, String)>,
}

impl PlayUrl {
    /// 来源 channel——派生自 [`PlayUrl::song_id`] 的 namespace。
    #[inline]
    pub fn source(&self) -> SourceKind {
        self.song_id.namespace()
    }
}

#[cfg(test)]
mod tests {
    use crate::bitrate::BitRate;
    use crate::format::AudioFormat;
    use crate::ids::SongId;
    use crate::play_url::PlayUrl;
    use crate::source::SourceKind;
    use crate::url::MediaUrl;

    /// stream_headers 跨 serde 往返:播放 URL 要携带如 Referer 的取流头(B站 baseUrl 播放必需),
    /// 经 IPC 序列化不能丢。
    #[test]
    fn stream_headers_survive_serde_roundtrip() -> color_eyre::Result<()> {
        let pu = PlayUrl {
            song_id: SongId::new(SourceKind::NETEASE, "1"),
            url: MediaUrl::remote("https://example.com/a.m4s")?,
            bitrate_bps: 320_000,
            quality: BitRate::Exhigh,
            size: 0,
            format: AudioFormat::Mp3,
            bit_depth: None,
            stream_headers: vec![("Referer".to_owned(), "https://www.bilibili.com".to_owned())],
        };
        let json = serde_json::to_string(&pu)?;
        let back = serde_json::from_str::<PlayUrl>(&json)?;
        assert_eq!(back.stream_headers, pu.stream_headers);
        Ok(())
    }
}
