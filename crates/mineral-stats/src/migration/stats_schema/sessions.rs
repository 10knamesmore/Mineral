//! sessions 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Sessions {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 开始时间，Unix 毫秒。
    StartedAt,
    /// 结束时间，Unix 毫秒。
    EndedAt,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Sessions::Table);
    table.col(
        ColumnDef::new(Sessions::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(Sessions::StartedAt).integer().not_null());
    table.col(ColumnDef::new(Sessions::EndedAt).integer().not_null());
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
