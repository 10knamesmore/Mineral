//! `mineral.ui.*`:脚本对用户界面的提示与标题覆盖出口。

pub(crate) mod card;
pub(crate) mod span;
mod table;
pub(crate) mod toast;
pub(crate) mod window_title;

pub(crate) use table::install;
