//! stats.db 存储层:句柄([`handle`])+ 事实写入([`write`])+ 事件写入([`event`])+
//! 维表维护([`songs`])+ 裁剪([`prune`])+ 聚合查询([`query`])+ 事件盘点([`summary`])。

mod event;
mod handle;
mod prune;
mod query;
mod songs;
mod summary;
mod write;

pub use handle::StatsStore;
pub use prune::is_event_kind;
