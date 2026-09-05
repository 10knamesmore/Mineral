//! song_stats 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SongStats {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌曲身份。
    SongValue,
    /// 播放次数。
    PlayCount,
    /// 跳过次数。
    SkipCount,
    /// 累计收听毫秒数。
    TotalListenMs,
    /// 最近播放时间，Unix 毫秒。
    LastPlayedAt,
    /// 用户评分。
    Rating,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SongStats::Table);
    table.col(ColumnDef::new(SongStats::Namespace).text().not_null());
    table.col(ColumnDef::new(SongStats::SongValue).text().not_null());
    table.col(
        ColumnDef::new(SongStats::PlayCount)
            .integer()
            .not_null()
            .default(0i64),
    );
    table.col(
        ColumnDef::new(SongStats::SkipCount)
            .integer()
            .not_null()
            .default(0i64),
    );
    table.col(
        ColumnDef::new(SongStats::TotalListenMs)
            .integer()
            .not_null()
            .default(0i64),
    );
    table.col(ColumnDef::new(SongStats::LastPlayedAt).integer());
    table.col(ColumnDef::new(SongStats::Rating).integer());
    table.primary_key(
        Index::create()
            .col(SongStats::Namespace)
            .col(SongStats::SongValue),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
