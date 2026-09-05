//! song_envelope 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SongEnvelope {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌曲身份。
    SongValue,
    /// 数据编码版本。
    Version,
    /// 包络点编码。
    Points,
    /// 最近更新时间，Unix 毫秒。
    UpdatedAt,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SongEnvelope::Table);
    table.col(ColumnDef::new(SongEnvelope::Namespace).text().not_null());
    table.col(ColumnDef::new(SongEnvelope::SongValue).text().not_null());
    table.col(ColumnDef::new(SongEnvelope::Version).integer().not_null());
    table.col(ColumnDef::new(SongEnvelope::Points).binary().not_null());
    table.col(ColumnDef::new(SongEnvelope::UpdatedAt).integer().not_null());
    table.primary_key(
        Index::create()
            .col(SongEnvelope::Namespace)
            .col(SongEnvelope::SongValue),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
