//! 此数据库的版本化迁移链。

use sea_orm_migration::{MigrationTrait, MigratorTrait};

/// TUI 数据库的迁移入口。
pub(crate) struct TuiMigrator;

#[async_trait::async_trait]
impl MigratorTrait for TuiMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(super::m20260906_client::Migration)]
    }
}
