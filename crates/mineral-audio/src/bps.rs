//! 万分比刻度类型:播放基建统一的整数比例表示。

use serde::{Deserialize, Serialize};

/// 万分比(basis points):`0..=10_000` 的整数比例,[`Self::FULL`](= 10_000)为满。
///
/// 统一全部 `*_bps` 字段的刻度:u16 可进无锁原子与 IPC,精度 0.01% 对进度条绰绰有余。
/// 构造一律经 [`Self::new`] / [`Self::ratio`] clamp,「≤ 满格」不变式由类型保证;
/// serde 透明(wire 上仍是裸 u16,bincode 字节不变)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Bps(u16);

impl Bps {
    /// 零比例。
    pub const ZERO: Self = Self(0);

    /// 满格(100%)。
    pub const FULL: Self = Self(10_000);

    /// 从裸值构造,超出满格 clamp。
    ///
    /// # Params:
    ///   - `raw`: 裸万分比值
    ///
    /// # Return:
    ///   clamp 进 `0..=10_000` 的比例。
    pub fn new(raw: u16) -> Self {
        Self(raw.min(Self::FULL.0))
    }

    /// 由「部分 / 总量」计算比例。
    ///
    /// # Params:
    ///   - `part`: 部分量(字节数 / 毫秒数等)
    ///   - `total`: 总量;`0` 表示未知,返回 [`Self::ZERO`]
    ///
    /// # Return:
    ///   比例;`part > total` clamp 到满,极大值经 saturating 不溢出。
    pub fn ratio(part: u64, total: u64) -> Self {
        if total == 0 {
            return Self::ZERO;
        }
        let bps = part.saturating_mul(10_000) / total;
        Self(u16::try_from(bps.min(10_000)).unwrap_or(Self::FULL.0))
    }

    /// 取裸值(喂原子存储等需要裸 u16 的边缘)。
    pub fn get(self) -> u16 {
        self.0
    }

    /// 是否满格(= 字节 / 进度全到齐)。
    pub fn is_full(self) -> bool {
        self == Self::FULL
    }

    /// 把比例应用到 `n` 份(如进度条 cell 数):`n * bps / 10_000`,纯整数无浮点。
    ///
    /// # Params:
    ///   - `n`: 总份数
    ///
    /// # Return:
    ///   占的份数(恒 ≤ `n`)。
    pub fn of(self, n: usize) -> usize {
        n.saturating_mul(usize::from(self.0)) / 10_000
    }
}

#[cfg(test)]
mod tests {
    use super::Bps;

    /// `new`:范围内原样,超界 clamp 到满。
    #[test]
    fn new_clamps_to_full() {
        assert_eq!(Bps::new(0), Bps::ZERO);
        assert_eq!(Bps::new(4_200).get(), 4_200);
        assert_eq!(Bps::new(10_000), Bps::FULL);
        assert_eq!(Bps::new(u16::MAX), Bps::FULL);
    }

    /// `ratio`:0 / 一半 / 满 / 超界 clamp;`total == 0`(长度未知)恒零。
    #[test]
    fn ratio_cases() {
        assert_eq!(Bps::ratio(0, 1000), Bps::ZERO);
        assert_eq!(Bps::ratio(500, 1000).get(), 5_000);
        assert_eq!(Bps::ratio(1000, 1000), Bps::FULL);
        // 部分超过总量(理论不该发生)clamp 到满,不溢出。
        assert_eq!(Bps::ratio(2000, 1000), Bps::FULL);
        // 总量未知:无法算比例,返回零(由调用方在完成时刻补满)。
        assert_eq!(Bps::ratio(123, 0), Bps::ZERO);
        assert_eq!(Bps::ratio(0, 0), Bps::ZERO);
    }

    /// 极大字节数不因 `* 10_000` 溢出 / panic,结果始终 ≤ 满格;现实 GB 量级整段缓冲 = 满格。
    #[test]
    fn ratio_no_overflow_on_huge_bytes() {
        // 病态量级(saturating_mul 兜底,不 panic);具体值无意义,只要 clamp 在范围内。
        assert!(Bps::ratio(u64::MAX, u64::MAX) <= Bps::FULL);
        assert!(Bps::ratio(u64::MAX, 1) <= Bps::FULL);
        // 现实量级(2 GB)整段下完 = 满格,saturating 不触发。
        assert_eq!(Bps::ratio(2_000_000_000, 2_000_000_000), Bps::FULL);
    }

    /// `of`:满格全占、零比例占 0、半比例占一半,结果恒 ≤ n。
    #[test]
    fn of_scales_within_n() {
        assert_eq!(Bps::FULL.of(8), 8);
        assert_eq!(Bps::ZERO.of(8), 0);
        assert_eq!(Bps::new(5_000).of(8), 4);
        // 病态量级 n:saturating 兜底不 panic,饱和后除回刻度(仍远小于 n)。
        assert_eq!(Bps::new(9_999).of(usize::MAX), usize::MAX / 10_000);
    }

    /// `is_full`:仅 10_000 为满。
    #[test]
    fn is_full_only_at_ten_thousand() {
        assert!(Bps::FULL.is_full());
        assert!(!Bps::new(9_999).is_full());
        assert!(!Bps::ZERO.is_full());
    }
}
