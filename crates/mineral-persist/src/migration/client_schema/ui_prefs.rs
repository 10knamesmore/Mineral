//! ui_prefs 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum UiPrefs {
    /// 数据表。
    Table,
    /// 记录键。
    Key,
    /// 保存的文本值。
    Value,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(UiPrefs::Table);
    table.col(ColumnDef::new(UiPrefs::Key).text().primary_key());
    table.col(ColumnDef::new(UiPrefs::Value).text().not_null());
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
