//! `mineral.tui` 独有的本地通知与窗口标题出口。

pub(crate) mod card;
pub(crate) mod span;
mod table;
pub(crate) mod toast;
pub(crate) mod window_title;

pub(crate) use table::install;
