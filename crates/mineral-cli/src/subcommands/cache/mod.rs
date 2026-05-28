//! `mineral cache` 子命令树:`status`(展示音频 / 封面 / 歌单缓存状态)与
//! `clean`(清理可重建缓存并展示清理效果)。两者都直接读存储,不经 daemon。

mod command;
mod render;

pub use command::{CacheCommand, run};
