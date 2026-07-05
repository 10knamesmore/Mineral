use serde::{Deserialize, Serialize};

use crate::{
    bitrate::BitRate, format::AudioFormat, ids::SongId, source::SourceKind, url::MediaUrl,
};

/// 流的容器布局:描述随机访问(seek)在**打开解码器时**的代价,供播放层选打开策略。
///
/// 这是**载荷自身的属性**(流的物理排布),不是来源身份;播放层据此决定 open 策略,来源无关。
/// 分片自适应容器(fMP4 / WebM-DASH,如 B站 / 未来 YouTube)在 seekable 模式下,解码器 open 时
/// 要扫遍所有分片建 seek 索引——本地文件很快,但**网络流等于开播前先把整段拉一遍**,起播被拖慢。
/// 故 [`Chunked`](Self::Chunked) 的远端流以流式打开(开播不预扫);落盘缓存后本地重播自然恢复全 seek。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StreamLayout {
    /// 整块 / 直链布局(MP3、直链 FLAC 等):随机访问廉价,可建完整 seek 索引。
    #[default]
    Contiguous,

    /// 分片自适应容器(fMP4 / WebM-DASH):建全 seek 索引需扫遍所有分片(网络流 = 拉整段),
    /// 故远端流以流式打开(开播不预扫,流式期间不支持向后 seek)。
    Chunked,
}

/// 一首歌的可播放 URL + 元信息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayUrl {
    /// 关联的歌曲 ID(自带 namespace)。
    pub song_id: SongId,
    /// 播放地址:远端 stream URL 用 `Remote`,本地文件用 `Local`。
    pub url: MediaUrl,
    /// 实际比特率(bps);`None` = 未知(接口没给且无从估算),显示侧据此省略码率段。
    pub bitrate_bps: Option<u32>,
    /// channel 解析后的归一化音质等级。
    pub quality: BitRate,
    /// 文件大小(bytes);`None` = 未知。
    pub size: Option<u64>,
    /// 文件格式——channel 实际提供的容器格式(`mp3` / `flac` 等);`None` = 未提供 / 探不出。
    pub format: Option<AudioFormat>,
    /// 位深(bits per sample),如 16 / 24。仅本地无损文件经实测有值;流式来源的接口不返回
    /// 位深、有损格式(mp3/aac)亦无此概念,这些情形均为 `None`(显示侧据此省略位深段)。
    pub bit_depth: Option<u8>,

    /// 取流时必须附加的请求头(如 B站 baseUrl 播放需 `Referer`);空 = 无附加头。键值对而非
    /// map:保序、允许重复头。随 `PlayUrl` 一起经 IPC 序列化,不能在播放/下载链路丢失。
    pub stream_headers: Vec<(String, String)>,

    /// 流的容器布局([`StreamLayout`]):播放层据此选解码器打开策略。缺省(旧载荷)→ `Contiguous`。
    #[serde(default)]
    pub layout: StreamLayout,

    /// 播放地址是否被拦截脚本顶换过(URL 非本曲 channel 所出)。展示层据此降低对
    /// 「借自原源的元数据」的信任(如歌词时间轴按实测时长差分档降级)。缺省(旧载荷)→ `false`。
    #[serde(default)]
    pub substituted: bool,
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
            bitrate_bps: Some(320_000),
            quality: BitRate::Exhigh,
            size: None,
            format: Some(AudioFormat::Mp3),
            bit_depth: None,
            stream_headers: vec![("Referer".to_owned(), "https://www.bilibili.com".to_owned())],
            layout: crate::play_url::StreamLayout::Contiguous,
            substituted: false,
        };
        let json = serde_json::to_string(&pu)?;
        let back = serde_json::from_str::<PlayUrl>(&json)?;
        assert_eq!(back.stream_headers, pu.stream_headers);
        Ok(())
    }

    /// StreamLayout 默认整块(Contiguous):未声明布局的源保持原 seekable 打开行为。
    #[test]
    fn stream_layout_defaults_contiguous() {
        assert_eq!(
            crate::play_url::StreamLayout::default(),
            crate::play_url::StreamLayout::Contiguous
        );
    }

    /// layout 跨 serde 往返:分片布局(B站 fMP4)经 IPC 不能丢,否则播放层退回 seekable 全扫、
    /// 起播被拖慢。
    #[test]
    fn layout_survives_serde_roundtrip() -> color_eyre::Result<()> {
        use crate::play_url::StreamLayout;

        let pu = PlayUrl {
            song_id: SongId::new(SourceKind::BILIBILI, "BV1x:1"),
            url: MediaUrl::remote("https://example.com/a.m4s")?,
            bitrate_bps: Some(192_000),
            quality: BitRate::Exhigh,
            size: None,
            format: Some(AudioFormat::Aac),
            bit_depth: None,
            stream_headers: Vec::new(),
            layout: StreamLayout::Chunked,
            substituted: false,
        };
        let back = serde_json::from_str::<PlayUrl>(&serde_json::to_string(&pu)?)?;
        assert_eq!(back.layout, StreamLayout::Chunked);
        Ok(())
    }
}
