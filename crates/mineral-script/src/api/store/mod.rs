//! `mineral.store.*`:per-song 持久 KV 的脚本出口。

pub(crate) mod get;
pub(crate) mod inc;
pub(crate) mod set;
mod table;

pub(crate) use table::install;
