//! playlist_entries 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum PlaylistEntries {
    /// 数据表。
    Table,
    /// 歌单来源。
    PlaylistNamespace,
    /// 来源内歌单身份。
    PlaylistValue,
    /// 歌单条目的原始位置。
    CollectionIndex,
    /// 歌曲来源。
    SongNamespace,
    /// 来源内歌曲身份。
    SongValue,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(PlaylistEntries::Table);
    table.col(
        ColumnDef::new(PlaylistEntries::PlaylistNamespace)
            .text()
            .not_null(),
    );
    table.col(
        ColumnDef::new(PlaylistEntries::PlaylistValue)
            .text()
            .not_null(),
    );
    table.col(
        ColumnDef::new(PlaylistEntries::CollectionIndex)
            .integer()
            .not_null(),
    );
    table.col(
        ColumnDef::new(PlaylistEntries::SongNamespace)
            .text()
            .not_null(),
    );
    table.col(ColumnDef::new(PlaylistEntries::SongValue).text().not_null());
    table.primary_key(
        Index::create()
            .col(PlaylistEntries::PlaylistNamespace)
            .col(PlaylistEntries::PlaylistValue)
            .col(PlaylistEntries::CollectionIndex),
    );
    table.check(Expr::col(PlaylistEntries::CollectionIndex).gte(Expr::value(0i64)));
    table.foreign_key(
        ForeignKey::create()
            .from(
                PlaylistEntries::Table,
                (
                    PlaylistEntries::PlaylistNamespace,
                    PlaylistEntries::PlaylistValue,
                ),
            )
            .to(
                super::playlist_cache::PlaylistCache::Table,
                (
                    super::playlist_cache::PlaylistCache::Namespace,
                    super::playlist_cache::PlaylistCache::PlaylistId,
                ),
            )
            .on_update(ForeignKeyAction::NoAction)
            .on_delete(ForeignKeyAction::Cascade),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
