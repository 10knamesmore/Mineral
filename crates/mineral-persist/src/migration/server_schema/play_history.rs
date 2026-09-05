//! play_history 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum PlayHistory {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌曲身份。
    SongValue,
    /// 播放发生时间，Unix 毫秒。
    PlayedAt,
    /// 是否完整播放。
    Completed,
    /// 实际收听毫秒数。
    ListenMs,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(PlayHistory::Table);
    table.col(
        ColumnDef::new(PlayHistory::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(PlayHistory::Namespace).text().not_null());
    table.col(ColumnDef::new(PlayHistory::SongValue).text().not_null());
    table.col(ColumnDef::new(PlayHistory::PlayedAt).integer().not_null());
    table.col(ColumnDef::new(PlayHistory::Completed).integer().not_null());
    table.col(ColumnDef::new(PlayHistory::ListenMs).integer().not_null());
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![
        Index::create()
            .name("idx_play_history_ns_time")
            .table(PlayHistory::Table)
            .col(PlayHistory::Namespace)
            .col(PlayHistory::PlayedAt)
            .to_owned(),
    ]
}
