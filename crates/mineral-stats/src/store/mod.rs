//! 播放事实、事件和歌曲维度的持久化及聚合查询。

mod event;
mod event_table;
mod handle;
mod prune;
mod query;
mod songs;
mod summary;
mod write;

pub use handle::StatsStore;
pub use prune::is_event_kind;
