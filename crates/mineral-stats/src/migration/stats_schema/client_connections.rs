//! client_connections 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum ClientConnections {
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
    /// 客户端类型。
    Client,
    /// 已知时长，单位毫秒。
    DurationMs,
    /// 并发连接数量。
    Concurrent,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(ClientConnections::Table);
    table.col(
        ColumnDef::new(ClientConnections::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(ClientConnections::Ts).integer().not_null());
    table.col(ColumnDef::new(ClientConnections::SessionId).integer());
    table.col(ColumnDef::new(ClientConnections::Actor).text().not_null());
    table.col(ColumnDef::new(ClientConnections::Client).text().not_null());
    table.col(
        ColumnDef::new(ClientConnections::DurationMs)
            .integer()
            .not_null(),
    );
    table.col(
        ColumnDef::new(ClientConnections::Concurrent)
            .integer()
            .not_null(),
    );
    table.check(Expr::col(ClientConnections::Actor).is_in(["user", "script", "system", "cli"]));
    table.foreign_key(
        ForeignKey::create()
            .from(ClientConnections::Table, ClientConnections::SessionId)
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
            .name("idx_client_connections_ts")
            .table(ClientConnections::Table)
            .col(ClientConnections::Ts)
            .to_owned(),
    ]
}
