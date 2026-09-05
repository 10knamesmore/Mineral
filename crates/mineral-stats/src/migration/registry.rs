//! 当前数据库的迁移链。

use sea_orm_migration::{MigrationTrait, MigratorTrait};

/// stats 数据库的迁移入口。
pub(crate) struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(super::m20260906_stats::Migration)]
    }
}
