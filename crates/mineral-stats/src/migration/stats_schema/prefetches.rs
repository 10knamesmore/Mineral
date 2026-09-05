//! prefetches 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Prefetches {
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
    /// 来源标识。
    Source,
    /// 预取结果。
    Resolution,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Prefetches::Table);
    table.col(
        ColumnDef::new(Prefetches::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(Prefetches::Ts).integer().not_null());
    table.col(ColumnDef::new(Prefetches::SessionId).integer());
    table.col(ColumnDef::new(Prefetches::Ns).text().not_null());
    table.col(ColumnDef::new(Prefetches::SongValue).text().not_null());
    table.col(ColumnDef::new(Prefetches::Source).text().not_null());
    table.col(ColumnDef::new(Prefetches::Resolution).text().not_null());
    table.check(Expr::col(Prefetches::Source).is_in(["local", "remote"]));
    table.check(Expr::col(Prefetches::Resolution).is_in([
        "armed",
        "vetoed",
        "rewritten",
        "failed",
    ]));
    table.foreign_key(
        ForeignKey::create()
            .from(Prefetches::Table, Prefetches::SessionId)
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
            .name("idx_prefetches_ts")
            .table(Prefetches::Table)
            .col(Prefetches::Ts)
            .to_owned(),
    ]
}
