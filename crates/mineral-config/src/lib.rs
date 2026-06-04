//! Mineral 运行期策略常量与配置基础类型的单一真相源。
//!
//! 目前住缓存容量上限与按键和弦类型,是未来配置系统的种子:容量从硬编码常量迁到此处集中,
//! 配置系统落地后改为从这里读取运行期配置,调用方(server / tui / cli)无需再各自硬编码。

pub mod keys;

/// 音频本体缓存容量上限:10 GiB。LRU 满了自动驱逐最久未播。
pub const AUDIO_CACHE_CAPACITY: u64 = 10 * 1024 * 1024 * 1024;

/// 封面磁盘缓存容量上限:1 GiB。LRU 满了自动驱逐最旧。
pub const COVER_CACHE_CAPACITY: u64 = 1024 * 1024 * 1024;
