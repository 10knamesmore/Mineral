//! `mineral.queue.*`:播放队列的脚本出口(读队列 + 整表重排)。

pub(crate) mod list;
pub(crate) mod set;
mod table;

pub(crate) use table::install;
