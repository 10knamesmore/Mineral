//! `mineral.player.*`:播放器控制命令族(全部 fire-and-forget)。

pub(crate) mod next;
pub(crate) mod play;
pub(crate) mod prev;
pub(crate) mod seek_rel;
pub(crate) mod seek_to;
pub(crate) mod set_mode;
pub(crate) mod set_volume;
pub(crate) mod stop;
mod table;
pub(crate) mod toggle;

#[cfg(test)]
mod tests;

pub(crate) use table::install;
