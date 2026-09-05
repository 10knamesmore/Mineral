//! songs 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Songs {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Ns,
    /// 来源内歌曲身份。
    SongValue,
    /// 展示名称。
    Name,
    /// 别名或译名。
    Alias,
    /// 来源内专辑身份。
    AlbumId,
    /// 专辑名称。
    AlbumName,
    /// 已知时长，单位毫秒。
    DurationMs,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Songs::Table);
    table.col(ColumnDef::new(Songs::Ns).text().not_null());
    table.col(ColumnDef::new(Songs::SongValue).text().not_null());
    table.col(ColumnDef::new(Songs::Name).text().not_null());
    table.col(ColumnDef::new(Songs::Alias).text());
    table.col(ColumnDef::new(Songs::AlbumId).text());
    table.col(ColumnDef::new(Songs::AlbumName).text());
    table.col(ColumnDef::new(Songs::DurationMs).integer());
    table.primary_key(Index::create().col(Songs::Ns).col(Songs::SongValue));
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
