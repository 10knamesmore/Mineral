//! searches 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Searches {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 事件时间，Unix 毫秒。
    Ts,
    /// 所属会话身份。
    SessionId,
    /// 行为发起方。
    Actor,
    /// 搜索文本。
    Query,
    /// 搜索文本摘要。
    QueryHash,
    /// 搜索目标类型。
    Kind,
    /// 来源标识。
    Source,
    /// 请求页码。
    Page,
    /// 结果数量。
    ResultCount,
    /// 操作结果。
    Outcome,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Searches::Table);
    table.col(
        ColumnDef::new(Searches::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(Searches::Ts).integer().not_null());
    table.col(ColumnDef::new(Searches::SessionId).integer());
    table.col(ColumnDef::new(Searches::Actor).text().not_null());
    table.col(ColumnDef::new(Searches::Query).text());
    table.col(ColumnDef::new(Searches::QueryHash).text().not_null());
    table.col(ColumnDef::new(Searches::Kind).text().not_null());
    table.col(ColumnDef::new(Searches::Source).text().not_null());
    table.col(ColumnDef::new(Searches::Page).integer().not_null());
    table.col(ColumnDef::new(Searches::ResultCount).integer());
    table.col(ColumnDef::new(Searches::Outcome).text().not_null());
    table.check(Expr::col(Searches::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(Searches::Kind).is_in(["song", "album", "artist", "playlist"]));
    table.check(Expr::col(Searches::Outcome).is_in(["ok", "failed", "cancelled"]));
    table.foreign_key(
        ForeignKey::create()
            .from(Searches::Table, Searches::SessionId)
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
            .name("idx_searches_ts")
            .table(Searches::Table)
            .col(Searches::Ts)
            .to_owned(),
    ]
}
