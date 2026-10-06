//! 服务端数据库的迁移入口和额外约束。

mod m20260906_server;
mod migrator;

pub(crate) use migrator::ServerMigrator;

mod m20260930_user_playlists;
mod m20261007_album_cache;
