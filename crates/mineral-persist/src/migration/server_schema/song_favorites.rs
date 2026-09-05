//! song_favorites 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, IndexOrder, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SongFavorites {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌曲身份。
    SongValue,
    /// 加入收藏的时间，Unix 毫秒。
    EnteredAt,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SongFavorites::Table);
    table.col(ColumnDef::new(SongFavorites::Namespace).text().not_null());
    table.col(ColumnDef::new(SongFavorites::SongValue).text().not_null());
    table.col(
        ColumnDef::new(SongFavorites::EnteredAt)
            .integer()
            .not_null(),
    );
    table.primary_key(
        Index::create()
            .col(SongFavorites::Namespace)
            .col(SongFavorites::SongValue),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![
        Index::create()
            .name("idx_song_favorites_order")
            .table(SongFavorites::Table)
            .col((SongFavorites::EnteredAt, IndexOrder::Desc))
            .col(SongFavorites::Namespace)
            .col(SongFavorites::SongValue)
            .to_owned(),
    ]
}
