//! 按文件内容探测音频属性与标签。

use std::io::{Read, Seek};

use derive_getters::Getters;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag};
use mineral_model::AudioFormat;

/// 从音频文件标签读出的元数据。
///
/// 全 `Option`:标签缺项就让类型说出来,不造空串哨兵。大小写 / 别名归一是消费侧的事。
#[derive(Clone, Debug, Default, PartialEq, Eq, Getters)]
pub struct ProbedTags {
    /// 曲名。
    title: Option<String>,

    /// 艺人(原始字符串,可能是「A / B」等多人合写,拆分归消费侧)。
    artist: Option<String>,

    /// 专辑名。
    album: Option<String>,

    /// 专辑艺人(合辑 / V.A. 场景用)。
    album_artist: Option<String>,

    /// 专辑内曲序(1-based)。
    track_no: Option<u32>,

    /// 流派(genre,自由文本)。
    genre: Option<String>,
}

/// 按文件内容探测出的音频信息:格式 / 码率 / 位深 / 时长 / 标签。
#[derive(Clone, Debug, PartialEq, Eq, Getters)]
pub struct ProbedAudio {
    /// 容器格式(未覆盖类型为 `None`)。
    format: Option<AudioFormat>,

    /// 码率(kbps;lofty 未提供为 `None`)。
    bitrate_kbps: Option<u32>,

    /// 位深(bit;仅无损容器有值)。
    bit_depth: Option<u8>,

    /// 时长(ms;lofty 判不出为 `None`,不拿 `0` 当哨兵)。
    duration_ms: Option<u64>,

    /// 标签元数据。
    tags: ProbedTags,
}

/// 按**文件内容**探测音频(经 lofty,全程不碰扩展名)。
///
/// 用 [`Probe::guess_file_type`](先跳 ID3 标签再认底层帧):[`FileType::from_buffer`] 对
/// 「ID3 标签 + MPEG 帧」结构(FFmpeg 转码的 mp3 恰是)在标签较大、帧头落在其扫描窗口外时会
/// 漏判成 `None`;`guess_file_type` 跳过标签再认帧,稳判得对。走 reader(非 path):调用方喂
/// 本地 `File` / storage backend 的读取器皆可;认不出即 `None`(不回退扩展名——那会与
/// 「按内容命名的缓存文件」循环依赖)。
///
/// # Params:
///   - `reader`: 音频字节流(需 `Read + Seek`,探测要随机访问)
///
/// # Return:
///   探测结果;打开 / 识别失败为 `None`。
pub fn probe<R: Read + Seek>(reader: R) -> Option<ProbedAudio> {
    let probe = Probe::new(reader).guess_file_type().ok()?;
    // 容器类型判不出 → 不是(已知)音频,None(乱字节走这里)。
    let format = file_type_to_format(probe.file_type()?);
    // 完整解析属性 + 标签;解析失败(损坏 / 不完整帧)时**降级为「仅格式已知」**——
    // 格式来自内容探测,不因 props/tags 读不出就把已识别的格式一并丢掉。
    let Ok(tagged) = probe.read() else {
        return Some(ProbedAudio {
            format,
            bitrate_kbps: None,
            bit_depth: None,
            duration_ms: None,
            tags: ProbedTags::default(),
        });
    };
    let props = tagged.properties();
    let duration = props.duration();
    // 时长 0 视作「判不出」——真实音频不会是 0,lofty 对某些容器拿不到时返回 ZERO。
    let duration_ms = if duration.is_zero() {
        None
    } else {
        u64::try_from(duration.as_millis()).ok()
    };
    let tags = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .map(read_tags)
        .unwrap_or_default();
    Some(ProbedAudio {
        format,
        bitrate_kbps: props.audio_bitrate(),
        bit_depth: props.bit_depth(),
        duration_ms,
        tags,
    })
}

/// 从 lofty [`Tag`] 读出统一标签(Accessor 常用项 + AlbumArtist)。
///
/// # Params:
///   - `tag`: lofty 解析出的标签
///
/// # Return:
///   映射后的 [`ProbedTags`](缺项为 `None`)。
fn read_tags(tag: &Tag) -> ProbedTags {
    ProbedTags {
        title: tag.title().map(std::borrow::Cow::into_owned),
        artist: tag.artist().map(std::borrow::Cow::into_owned),
        album: tag.album().map(std::borrow::Cow::into_owned),
        album_artist: tag.get_string(&ItemKey::AlbumArtist).map(str::to_owned),
        track_no: tag.track(),
        genre: tag.genre().map(std::borrow::Cow::into_owned),
    }
}

/// lofty 容器类型 → model 的 [`AudioFormat`]。未覆盖类型为 `None`(格式未知)。
///
/// # Params:
///   - `ft`: lofty 探测出的文件类型
///
/// # Return:
///   对应的 [`AudioFormat`];未覆盖类型 `None`。
pub fn file_type_to_format(ft: FileType) -> Option<AudioFormat> {
    match ft {
        FileType::Mpeg => Some(AudioFormat::Mp3),
        FileType::Flac => Some(AudioFormat::Flac),
        FileType::Mp4 => Some(AudioFormat::Aac),
        FileType::Vorbis => Some(AudioFormat::Ogg),
        FileType::Wav => Some(AudioFormat::Wav),
        FileType::Ape => Some(AudioFormat::Ape),
        FileType::Aac => Some(AudioFormat::Aac),
        FileType::Opus => Some(AudioFormat::Other("opus".to_owned())),
        _ => None,
    }
}

/// 是否已知音频文件扩展名(大小写不敏感)。借此在探测前快速排除 `.part` / `.jpg` 等非音频。
///
/// # Params:
///   - `ext`: 扩展名(不含点)
///
/// # Return:
///   是音频扩展名返回 `true`。
pub fn is_audio_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp3" | "flac" | "aac" | "m4a" | "ogg" | "opus" | "wav" | "ape" | "alac"
    )
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use mineral_model::AudioFormat;

    use super::{file_type_to_format, is_audio_ext, probe};

    /// 合法最小 WAV(44B 头 + `data_len` 个 0 PCM):8000Hz / 8bit / 单声道 → lofty 算 64kbps。
    fn wav_bytes(data_len: usize) -> Vec<u8> {
        let data = u32::try_from(data_len).unwrap_or(0);
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&36u32.saturating_add(data).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        v.resize(v.len() + data_len, 0u8);
        v
    }

    /// 「ID3v2.4 标签 + 一个 MPEG-1 Layer III 帧」的最小 mp3:首字节是 `ID3`,MPEG 同步头在标签之后。
    /// 复刻 FFmpeg 转码产物——`from_buffer` 见 ID3 前缀即漏判、必须走 `Probe` 的场景。
    fn id3_prefixed_mp3() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"ID3");
        v.extend_from_slice(&[0x04, 0x00, 0x00]);
        v.extend_from_slice(&[0x00, 0x00, 0x00, 0x23]);
        v.resize(v.len() + 35, 0u8);
        v.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        v.resize(v.len() + 417 - 4, 0u8);
        v
    }

    /// WAV 内容按**内容**判 WAV,码率来自 lofty 解析(64kbps),位深 8bit。
    #[test]
    fn probe_wav_reads_props_from_content() -> color_eyre::Result<()> {
        let probed =
            probe(Cursor::new(wav_bytes(8000))).ok_or_else(|| color_eyre::eyre::eyre!("应探出"))?;
        assert_eq!(probed.format(), &Some(AudioFormat::Wav));
        assert_eq!(probed.bitrate_kbps(), &Some(64), "lofty 解析 64kbps");
        assert_eq!(probed.bit_depth(), &Some(8), "WAV 无损有位深");
        Ok(())
    }

    /// ID3 前缀的 mp3(首字节是 `ID3`、MPEG 同步头在标签之后)经 `probe` 判 Mp3——
    /// `guess_file_type` 跳标签再认帧。合成帧无有效帧体,`read()` 解析属性失败,走**降级路径**:
    /// 格式(来自内容探测)仍在,props 为 `None`——格式不因属性读不出而丢。
    #[test]
    fn probe_id3_prefixed_mp3_detects_mp3_format_survives_unparsable_frame() -> color_eyre::Result<()>
    {
        let probed = probe(Cursor::new(id3_prefixed_mp3()))
            .ok_or_else(|| color_eyre::eyre::eyre!("格式可探,应返回 Some"))?;
        assert_eq!(probed.format(), &Some(AudioFormat::Mp3), "格式来自内容探测,降级路径仍在");
        assert_eq!(probed.bitrate_kbps(), &None, "合成残帧属性读不出,props 为 None");
        Ok(())
    }

    /// 乱字节识别不出 → None。
    #[test]
    fn probe_garbage_is_none() {
        assert!(probe(Cursor::new(vec![0u8; 128])).is_none());
    }

    /// file_type_to_format 覆盖已知容器。
    #[test]
    fn file_type_maps_known_containers() {
        use lofty::file::FileType;
        assert_eq!(file_type_to_format(FileType::Mpeg), Some(AudioFormat::Mp3));
        assert_eq!(file_type_to_format(FileType::Flac), Some(AudioFormat::Flac));
        assert_eq!(file_type_to_format(FileType::Wav), Some(AudioFormat::Wav));
    }

    /// is_audio_ext 大小写不敏感,排除非音频。
    #[test]
    fn audio_ext_case_insensitive() {
        assert!(is_audio_ext("FLAC"));
        assert!(is_audio_ext("mp3"));
        assert!(!is_audio_ext("part"));
        assert!(!is_audio_ext("jpg"));
    }
}
