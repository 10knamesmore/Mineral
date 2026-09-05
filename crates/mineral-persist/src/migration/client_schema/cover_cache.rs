//! cover_cache 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum CoverCache {
    /// 数据表。
    Table,
    /// 记录键。
    Key,
    /// 相对缓存根目录的路径。
    Relpath,
    /// 文件字节数。
    Bytes,
    /// 最近访问的逻辑时钟。
    LastAccess,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(CoverCache::Table);
    table.col(ColumnDef::new(CoverCache::Key).text().primary_key());
    table.col(ColumnDef::new(CoverCache::Relpath).text().not_null());
    table.col(ColumnDef::new(CoverCache::Bytes).integer().not_null());
    table.col(ColumnDef::new(CoverCache::LastAccess).integer().not_null());
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
