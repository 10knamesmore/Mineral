//! `config` 子命令树:生成 / 校验用户配置。

mod command;
mod init;
mod load;

pub(crate) use load::daemon as load_daemon;

pub use command::{ConfigCommand, run};
