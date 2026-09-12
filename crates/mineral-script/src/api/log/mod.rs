//! `mineral.log.*`:脚本写日志。

pub(crate) mod info;
mod table;
pub(crate) mod warn;

#[cfg(test)]
mod tests;

pub(crate) use table::install;
