//! Mineral 用户配置(Lua):强类型 [`Config`] 的单一真相源。
//!
//! 内置 `default.lua` 经 `include_str!` 编入二进制,启动期与用户 `config.lua` 深合并后
//! 整表落成 [`Config`]。声明面(主题 / 键位 / 音频 / 缓存等)经 getter 读取;事件 hooks
//! 等可编程层的 host API 在此只有 no-op stub([`inject_noop_host`]),活实现由 daemon
//! 脚本运行时注入。键字符串与语义键的统一表示见 [`keys`]。

pub mod keys;

mod check;
mod init;
mod loader;
mod schema;

pub use check::render_check;
pub use init::{InitOutcome, run_init};
pub use loader::{ConfigWarning, inject_noop_host, load};
pub use schema::*;

/// 音频本体缓存容量上限:10 GiB。LRU 满了自动驱逐最久未播。
///
/// 过渡常量:与 `default.lua` 的 `cache.audio_capacity` 同值(守卫测试钉死),
/// 供接线前的旧调用方;接线 PR 改读 [`Config`] 后删除。
pub const AUDIO_CACHE_CAPACITY: u64 = 10 * 1024 * 1024 * 1024;

/// 封面磁盘缓存容量上限:1 GiB。LRU 满了自动驱逐最旧。
///
/// 过渡常量:与 `default.lua` 的 `cache.cover_capacity` 同值(守卫测试钉死),
/// 供接线前的旧调用方;接线 PR 改读 [`Config`] 后删除。
pub const COVER_CACHE_CAPACITY: u64 = 1024 * 1024 * 1024;
