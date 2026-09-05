//! bus_messages 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum BusMessages {
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
    /// 展示名称。
    Name,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(BusMessages::Table);
    table.col(
        ColumnDef::new(BusMessages::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(BusMessages::Ts).integer().not_null());
    table.col(ColumnDef::new(BusMessages::SessionId).integer());
    table.col(ColumnDef::new(BusMessages::Actor).text().not_null());
    table.col(ColumnDef::new(BusMessages::Name).text().not_null());
    table.check(Expr::col(BusMessages::Actor).is_in(["user", "script", "system", "cli"]));
    table.foreign_key(
        ForeignKey::create()
            .from(BusMessages::Table, BusMessages::SessionId)
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
            .name("idx_bus_messages_ts")
            .table(BusMessages::Table)
            .col(BusMessages::Ts)
            .to_owned(),
    ]
}
