use serde::{Deserialize, Serialize};

/// 跨 channel 的最小公约数音质枚举。
///
/// channel-only 的更高规格(网易的 JYEffect / Sky / JYMaster 等)在本枚举不暴露,
/// 由各 channel 实现内部归一化到这套值之一(就近映射或拒绝)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BitRate {
    Standard,
    #[default]
    Higher,
    Exhigh,
    Lossless,
    Hires,
}
