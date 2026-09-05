//! 数据库结构的版本化迁移。

mod client_schema;
mod m20260906_client;
mod m20260906_server;
mod registry;
mod server_schema;

pub(crate) use registry::{ClientMigrator, ServerMigrator};
