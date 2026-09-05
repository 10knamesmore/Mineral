//! playlist_cache 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum PlaylistCache {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌单身份。
    PlaylistId,
    /// 展示名称。
    Name,
    /// 取回时间，Unix 毫秒。
    FetchedAt,
    /// 来源提供的曲目更新时间。
    TrackUpdateTime,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(PlaylistCache::Table);
    table.col(ColumnDef::new(PlaylistCache::Namespace).text().not_null());
    table.col(ColumnDef::new(PlaylistCache::PlaylistId).text().not_null());
    table.col(ColumnDef::new(PlaylistCache::Name).text());
    table.col(
        ColumnDef::new(PlaylistCache::FetchedAt)
            .integer()
            .not_null(),
    );
    table.col(ColumnDef::new(PlaylistCache::TrackUpdateTime).integer());
    table.primary_key(
        Index::create()
            .col(PlaylistCache::Namespace)
            .col(PlaylistCache::PlaylistId),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
