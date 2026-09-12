//! 约束模块入口文件只承担模块声明与重导出。

mod rule;
#[cfg(test)]
mod tests;

pub(crate) use rule::{MINERAL_MOD_RS_ITEMS, ModRsItems};
