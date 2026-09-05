//! cache_harvests 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum CacheHarvests {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 事件时间，Unix 毫秒。
    Ts,
    /// 所属会话身份。
    SessionId,
    /// 来源稳定名。
    Ns,
    /// 来源内歌曲身份。
    SongValue,
    /// 音质标识。
    Quality,
    /// 音频格式。
    Format,
    /// 操作结果。
    Outcome,
    /// 文件字节数。
    Bytes,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(CacheHarvests::Table);
    table.col(
        ColumnDef::new(CacheHarvests::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(CacheHarvests::Ts).integer().not_null());
    table.col(ColumnDef::new(CacheHarvests::SessionId).integer());
    table.col(ColumnDef::new(CacheHarvests::Ns).text().not_null());
    table.col(ColumnDef::new(CacheHarvests::SongValue).text().not_null());
    table.col(ColumnDef::new(CacheHarvests::Quality).text().not_null());
    table.col(ColumnDef::new(CacheHarvests::Format).text().not_null());
    table.col(ColumnDef::new(CacheHarvests::Outcome).text().not_null());
    table.col(ColumnDef::new(CacheHarvests::Bytes).integer());
    table.check(Expr::col(CacheHarvests::Outcome).is_in(["cached", "discarded"]));
    table.foreign_key(
        ForeignKey::create()
            .from(CacheHarvests::Table, CacheHarvests::SessionId)
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
            .name("idx_cache_harvests_ts")
            .table(CacheHarvests::Table)
            .col(CacheHarvests::Ts)
            .to_owned(),
    ]
}
