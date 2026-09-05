//! song_meta 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SongMeta {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌曲身份。
    SongValue,
    /// 展示名称。
    Name,
    /// 来源内专辑身份。
    AlbumId,
    /// 专辑名称。
    AlbumName,
    /// 已知时长，单位毫秒。
    DurationMs,
    /// 封面地址。
    CoverUrl,
    /// 别名或译名。
    Alias,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SongMeta::Table);
    table.col(ColumnDef::new(SongMeta::Namespace).text().not_null());
    table.col(ColumnDef::new(SongMeta::SongValue).text().not_null());
    table.col(ColumnDef::new(SongMeta::Name).text().not_null());
    table.col(ColumnDef::new(SongMeta::AlbumId).text());
    table.col(ColumnDef::new(SongMeta::AlbumName).text());
    table.col(ColumnDef::new(SongMeta::DurationMs).integer());
    table.col(ColumnDef::new(SongMeta::CoverUrl).text());
    table.col(ColumnDef::new(SongMeta::Alias).text());
    table.primary_key(
        Index::create()
            .col(SongMeta::Namespace)
            .col(SongMeta::SongValue),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
