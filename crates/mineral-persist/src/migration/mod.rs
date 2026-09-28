//! 按数据库生命周期分开的版本化迁移。

mod client;
mod server;

pub(crate) use client::ClientMigrator;
pub(crate) use server::ServerMigrator;
