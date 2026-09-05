//! cache_evictions 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, ForeignKey, ForeignKeyAction, Iden, Index, IndexCreateStatement, Table,
    TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum CacheEvictions {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 事件时间，Unix 毫秒。
    Ts,
    /// 所属会话身份。
    SessionId,
    /// 缓存键。
    CacheKey,
    /// 文件字节数。
    Bytes,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(CacheEvictions::Table);
    table.col(
        ColumnDef::new(CacheEvictions::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(CacheEvictions::Ts).integer().not_null());
    table.col(ColumnDef::new(CacheEvictions::SessionId).integer());
    table.col(ColumnDef::new(CacheEvictions::CacheKey).text().not_null());
    table.col(ColumnDef::new(CacheEvictions::Bytes).integer().not_null());
    table.foreign_key(
        ForeignKey::create()
            .from(CacheEvictions::Table, CacheEvictions::SessionId)
            .to(
                super::sessions::Sessions::Table,
                super::sessions::Sessions::Id,
            )
            .on_update(ForeignKeyAction::NoAction)
            .on_delete(ForeignKeyAction::NoAction),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![
        Index::create()
            .name("idx_cache_evictions_ts")
            .table(CacheEvictions::Table)
            .col(CacheEvictions::Ts)
            .to_owned(),
    ]
}
