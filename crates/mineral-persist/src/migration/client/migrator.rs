//! 此数据库的版本化迁移链。

use sea_orm_migration::{MigrationTrait, MigratorTrait};

/// client 数据库的迁移入口。
pub(crate) struct ClientMigrator;

#[async_trait::async_trait]
impl MigratorTrait for ClientMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(super::m20260906_client::Migration)]
    }
}
