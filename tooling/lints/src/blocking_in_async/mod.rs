//! 检查异步执行体中的已配置阻塞调用。

mod calls;
mod config;
mod rule;

#[cfg(test)]
mod tests;

pub(crate) use config::Config;
pub(crate) use rule::{BlockingInAsync, MINERAL_BLOCKING_IN_ASYNC};
