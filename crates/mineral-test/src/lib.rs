//! Mineral 内部测试辅助库。
//!
//! 收口可**跨 crate 复用**的测试零件,避免各 crate 各抄一份:
//! - 快照断言宏 [`assert_snap!`] / [`assert_snap_debug!`](强制带中文 `description`)。
//! - [`Song`](mineral_model::Song) 构造器 [`song`] + 函数式装饰 [`with_artist`] /
//!   [`with_source`] / [`with_duration`]。
//! - 展示性 fixtures [`endserenading`] / [`chinese_football`] / [`qianzai_lyrics`]。
//! - proptest 生成器 [`arb_song`]。
//!
//! 用法:作为各 crate 的 **dev-dependency** 引入。仅 crate-private 的测试零件(依赖某
//! crate 内部类型的,如 TUI 的 `AppState` fixture)仍留在各 crate 自己的 `test_support`。

mod builders;
mod fixtures;
mod lyrics;
mod strategies;

// macros 模块里的宏经 `#[macro_export]` 挂在 crate 根,无需在此 re-export。
mod macros;

pub use builders::{song, with_artist, with_duration, with_name, with_source};
pub use fixtures::{chinese_football, endserenading};
pub use lyrics::{feiyu_lyrics, feiyu_song, qianzai_lyrics, qianzai_song};
pub use strategies::arb_song;
