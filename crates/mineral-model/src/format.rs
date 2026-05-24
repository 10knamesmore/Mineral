use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 音频容器格式。
///
/// 「边缘序列化、内部结构化」:channel wire 层以字符串接收(各家命名不一、且可能
/// 「尽力提供」返回意料外的值),进 model 时归一化到本枚举。未识别的值**保留原文**
/// 落入 [`AudioFormat::Other`],既不丢信息也不会反序列化失败。
///
/// serde 经 `from = "String"` / `into = "String"` 把本枚举当**纯字符串**进出,
/// 所以序列化结果与旧的 `String` 字段完全一致——wire 协议不变。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum AudioFormat {
    /// MP3(有损)。
    Mp3,
    /// FLAC(无损)。
    Flac,
    /// AAC / M4A(有损)。
    Aac,
    /// Ogg Vorbis(有损)。
    Ogg,
    /// WAV(无损 PCM)。
    Wav,
    /// Monkey's Audio(无损)。
    Ape,
    /// Apple Lossless(无损)。
    Alac,
    /// 未识别格式(保留 channel 原文)或缺失(空串)。
    Other(String),
}

impl AudioFormat {
    /// 规范化的格式名(固定变体返回 `'static`,`Other` 返回内部原文)。
    pub fn as_str(&self) -> &str {
        match self {
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Aac => "aac",
            Self::Ogg => "ogg",
            Self::Wav => "wav",
            Self::Ape => "ape",
            Self::Alac => "alac",
            Self::Other(s) => s.as_str(),
        }
    }

    /// 是否无损格式——供显示层做音质分级配色(不依赖请求侧的 quality)。
    pub fn is_lossless(&self) -> bool {
        matches!(self, Self::Flac | Self::Wav | Self::Ape | Self::Alac)
    }

    /// 格式缺失(channel 没提供)。
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Other(s) if s.is_empty())
    }
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self::Other(String::new())
    }
}

impl From<String> for AudioFormat {
    fn from(s: String) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "mp3" => Self::Mp3,
            "flac" => Self::Flac,
            "aac" | "m4a" => Self::Aac,
            "ogg" | "vorbis" => Self::Ogg,
            "wav" => Self::Wav,
            "ape" => Self::Ape,
            "alac" => Self::Alac,
            _ => Self::Other(s),
        }
    }
}

impl From<AudioFormat> for String {
    fn from(f: AudioFormat) -> Self {
        match f {
            AudioFormat::Other(s) => s,
            other => other.as_str().to_owned(),
        }
    }
}

impl FromStr for AudioFormat {
    type Err = Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s.to_owned()))
    }
}

impl fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_formats_roundtrip_lowercase() {
        assert_eq!(AudioFormat::from("FLAC".to_owned()), AudioFormat::Flac);
        assert_eq!(AudioFormat::Flac.as_str(), "flac");
        assert_eq!(String::from(AudioFormat::Mp3), "mp3");
    }

    #[test]
    fn unknown_format_preserved() {
        let f = AudioFormat::from("dsd".to_owned());
        assert_eq!(f, AudioFormat::Other("dsd".to_owned()));
        assert_eq!(f.as_str(), "dsd");
        assert!(!f.is_lossless());
    }

    #[test]
    fn lossless_classification() {
        assert!(AudioFormat::Flac.is_lossless());
        assert!(AudioFormat::Wav.is_lossless());
        assert!(!AudioFormat::Mp3.is_lossless());
    }

    #[test]
    fn empty_is_empty() {
        assert!(AudioFormat::default().is_empty());
        assert!(AudioFormat::from(String::new()).is_empty());
        assert!(!AudioFormat::Mp3.is_empty());
    }

    #[test]
    fn serde_is_plain_string() -> color_eyre::Result<()> {
        // 序列化结果应与旧 String 字段一致(纯字符串,非 tagged enum)。
        assert_eq!(serde_json::to_string(&AudioFormat::Flac)?, "\"flac\"");
        assert_eq!(
            serde_json::from_str::<AudioFormat>("\"mp3\"")?,
            AudioFormat::Mp3
        );
        assert_eq!(
            serde_json::from_str::<AudioFormat>("\"dsd\"")?,
            AudioFormat::Other("dsd".to_owned())
        );
        Ok(())
    }

    use proptest::prelude::*;
    use proptest::sample::select;

    /// 已知 token(小写)。`Other` 不该撞这些,否则归一化会把它吃成固定变体、破坏往返。
    fn is_known_token(s: &str) -> bool {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "mp3" | "flac" | "aac" | "m4a" | "ogg" | "vorbis" | "wav" | "ape" | "alac"
        )
    }

    /// 7 个固定变体(规范名均为小写 ascii)。
    fn arb_known_format() -> impl Strategy<Value = AudioFormat> {
        select(vec![
            AudioFormat::Mp3,
            AudioFormat::Flac,
            AudioFormat::Aac,
            AudioFormat::Ogg,
            AudioFormat::Wav,
            AudioFormat::Ape,
            AudioFormat::Alac,
        ])
    }

    /// 任意 `AudioFormat`:固定变体 + 不撞已知 token 的 `Other`(含空串)。
    fn arb_audio_format() -> impl Strategy<Value = AudioFormat> {
        prop_oneof![
            arb_known_format(),
            "[a-zA-Z0-9]{0,8}"
                .prop_filter("不撞已知 token", |s| !is_known_token(s))
                .prop_map(AudioFormat::Other),
        ]
    }

    proptest! {
        /// String 进出往返:`from(into(f)) == f`。wire 字段就是 String,等价于协议往返。
        #[test]
        fn prop_string_round_trip(f in arb_audio_format()) {
            prop_assert_eq!(AudioFormat::from(String::from(f.clone())), f);
        }

        /// serde 往返(纯字符串编码,非 tagged)。
        #[test]
        fn prop_serde_round_trip(f in arb_audio_format()) {
            let json =
                serde_json::to_string(&f).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let back = serde_json::from_str::<AudioFormat>(&json)
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(back, f);
        }

        /// 任意字符串输入既不 panic,归一化又幂等:`from(from(s).as_str()) == from(s)`。
        #[test]
        fn prop_from_arbitrary_idempotent(s in ".*") {
            let f = AudioFormat::from(s);
            prop_assert_eq!(AudioFormat::from(f.as_str().to_owned()), f);
        }

        /// 固定变体大小写无关:规范名翻成大写仍映射回同一变体。
        #[test]
        fn prop_known_case_insensitive(f in arb_known_format()) {
            prop_assert_eq!(AudioFormat::from(f.as_str().to_ascii_uppercase()), f);
        }

        /// 别名收敛:`m4a`→Aac、`vorbis`→Ogg(大小写无关),与规范名同归一。
        #[test]
        fn prop_aliases_map((input, expected) in select(vec![
            ("m4a", AudioFormat::Aac),
            ("M4A", AudioFormat::Aac),
            ("aac", AudioFormat::Aac),
            ("vorbis", AudioFormat::Ogg),
            ("VORBIS", AudioFormat::Ogg),
            ("ogg", AudioFormat::Ogg),
        ])) {
            prop_assert_eq!(AudioFormat::from(input.to_owned()), expected);
        }

        /// 无损分级只认这 4 个固定变体,任何 `Other`(未识别)恒为有损。
        #[test]
        fn prop_lossless_iff_known_lossless(f in arb_audio_format()) {
            let expected = matches!(
                f,
                AudioFormat::Flac | AudioFormat::Wav | AudioFormat::Ape | AudioFormat::Alac
            );
            prop_assert_eq!(f.is_lossless(), expected);
        }
    }
}
