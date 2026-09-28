//! TUI 客户端数据库的迁移链、版本脚本与表结构快照。

pub(super) mod cover_cache;
pub(super) mod track_pos;
pub(super) mod ui_prefs;

mod m20260906_client;
mod migrator;

pub(crate) use migrator::ClientMigrator;
