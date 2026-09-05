//! connection_rejects 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum ConnectionRejects {
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
    /// 拒绝原因。
    Reason,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(ConnectionRejects::Table);
    table.col(
        ColumnDef::new(ConnectionRejects::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(ConnectionRejects::Ts).integer().not_null());
    table.col(ColumnDef::new(ConnectionRejects::SessionId).integer());
    table.col(ColumnDef::new(ConnectionRejects::Actor).text().not_null());
    table.col(ColumnDef::new(ConnectionRejects::Reason).text().not_null());
    table.check(Expr::col(ConnectionRejects::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(ConnectionRejects::Reason).is_in(["busy", "version_mismatch"]));
    table.foreign_key(
        ForeignKey::create()
            .from(ConnectionRejects::Table, ConnectionRejects::SessionId)
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
            .name("idx_connection_rejects_ts")
            .table(ConnectionRejects::Table)
            .col(ConnectionRejects::Ts)
            .to_owned(),
    ]
}
