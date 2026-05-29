use serde::{Deserialize, Serialize};

/// 跨 channel 的最小公约数音质枚举。
///
/// channel-only 的更高规格(网易的 JYEffect / Sky / JYMaster 等)在本枚举不暴露,
/// 由各 channel 实现内部归一化到这套值之一(就近映射或拒绝)。
///
/// `Ord` 语义 = **音频质量升序**(`Standard < Higher < Exhigh < Lossless < Hires`),由
/// variant 声明顺序保证;调用方据此比较「本地副本是否够格」。新增 variant 务必按质量插对位置。
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
