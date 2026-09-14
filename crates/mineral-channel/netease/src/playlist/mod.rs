//! 网易歌单按需加载：浏览逐批检查身份，整张操作分批补齐所有缺失详情。

mod cache;
mod loader;

pub(crate) use loader::PlaylistLoader;
