//! TUI 数据库的迁移入口。

mod m20260906_client;
mod migrator;

pub(crate) use migrator::TuiMigrator;
