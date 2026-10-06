//! daemon session 配置与 TUI 本地配置共用的覆盖解析出口。

pub(crate) mod override_;
mod table;

pub(crate) use table::install;
