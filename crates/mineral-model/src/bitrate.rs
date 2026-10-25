use serde::{Deserialize, Serialize};

/// 跨 channel 的音质偏好档位。
///
/// channel 尽力选择接近偏好的可播资源, 来源特有规格由 channel 内部选择
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum BitRate {
    /// 标准音质(~128 kbps)。
    Standard,

    /// 较高音质(~190 kbps)。
    Higher,

    /// 极高音质(~320 kbps)。
    #[default]
    Exhigh,

    /// 无损音质(FLAC)。
    Lossless,

    /// Hi-Res 音质(>= 24bit/96kHz)。
    Hires,
}

impl BitRate {
    /// 全部音质,按质量**升序**(与 `Ord` 一致)。新增 variant 须同步维护此数组顺序。
    pub const ALL: [Self; 5] = [
        Self::Standard,
        Self::Higher,
        Self::Exhigh,
        Self::Lossless,
        Self::Hires,
    ];

    /// 稳定的小写 token,用作缓存键 / 目录名(与 serde `lowercase` 一致)。
    ///
    /// # Return:
    ///   `'static` 小写名,如 `lossless`。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Higher => "higher",
            Self::Exhigh => "exhigh",
            Self::Lossless => "lossless",
            Self::Hires => "hires",
        }
    }

    /// [`as_str`](Self::as_str) 的反解析(缓存键 / 目录名 → 音质)。
    ///
    /// # Params:
    ///   - `token`: 小写 token(如 `lossless`)
    ///
    /// # Return:
    ///   命中的音质;未知 token 为 `None`。
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.as_str() == token)
    }
}

#[cfg(test)]
mod tests {
    use super::BitRate;

    #[test]
    fn as_str_is_stable_lowercase() {
        assert_eq!(BitRate::Lossless.as_str(), "lossless");
        assert_eq!(BitRate::Exhigh.as_str(), "exhigh");
        assert_eq!(BitRate::Standard.as_str(), "standard");
        assert_eq!(BitRate::Higher.as_str(), "higher");
        assert_eq!(BitRate::Hires.as_str(), "hires");
    }

    /// `as_str` / `from_token` 互逆。
    #[test]
    fn from_token_roundtrips() {
        for b in BitRate::ALL {
            assert_eq!(BitRate::from_token(b.as_str()), Some(b));
        }
        assert_eq!(BitRate::from_token("master"), None);
    }

    /// Ord 必须是音频质量升序,解析器据此「取最高可用音质」。
    #[test]
    fn ord_is_ascending_quality() {
        assert!(BitRate::Standard < BitRate::Higher);
        assert!(BitRate::Higher < BitRate::Exhigh);
        assert!(BitRate::Exhigh < BitRate::Lossless);
        assert!(BitRate::Lossless < BitRate::Hires);
        // ALL 与 Ord 同序(升序)。
        let mut sorted = BitRate::ALL;
        sorted.sort();
        assert_eq!(sorted, BitRate::ALL);
    }
}
