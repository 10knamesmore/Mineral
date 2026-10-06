//! 此数据库的版本化迁移链。

use sea_orm_migration::{MigrationTrait, MigratorTrait};

/// server 数据库的迁移入口。
pub(crate) struct ServerMigrator;

#[async_trait::async_trait]
impl MigratorTrait for ServerMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(super::m20260906_server::Migration),
            Box::new(super::m20260930_user_playlists::Migration),
            Box::new(super::m20261007_album_cache::Migration),
        ]
    }
}
