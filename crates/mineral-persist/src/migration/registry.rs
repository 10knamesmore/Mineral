//! 当前数据库的迁移链。

use sea_orm_migration::{MigrationTrait, MigratorTrait};

/// server 数据库的迁移入口。
pub(crate) struct ServerMigrator;

#[async_trait::async_trait]
impl MigratorTrait for ServerMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(super::m20260906_server::Migration)]
    }
}

/// client 数据库的迁移入口。
pub(crate) struct ClientMigrator;

#[async_trait::async_trait]
impl MigratorTrait for ClientMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(super::m20260906_client::Migration)]
    }
}
