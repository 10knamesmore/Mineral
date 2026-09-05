//! session_queue 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SessionQueue {
    /// 数据表。
    Table,
    /// 原始排列位置。
    Position,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌曲身份。
    SongValue,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SessionQueue::Table);
    table.col(
        ColumnDef::new(SessionQueue::Position)
            .integer()
            .primary_key(),
    );
    table.col(ColumnDef::new(SessionQueue::Namespace).text().not_null());
    table.col(ColumnDef::new(SessionQueue::SongValue).text().not_null());
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
