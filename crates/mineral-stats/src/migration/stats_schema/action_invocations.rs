//! action_invocations 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum ActionInvocations {
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
    /// 触发方式。
    Trigger,
    /// 操作结果。
    Outcome,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(ActionInvocations::Table);
    table.col(
        ColumnDef::new(ActionInvocations::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(ActionInvocations::Ts).integer().not_null());
    table.col(ColumnDef::new(ActionInvocations::SessionId).integer());
    table.col(ColumnDef::new(ActionInvocations::Actor).text().not_null());
    table.col(ColumnDef::new(ActionInvocations::Name).text().not_null());
    table.col(ColumnDef::new(ActionInvocations::Trigger).text().not_null());
    table.col(ColumnDef::new(ActionInvocations::Outcome).text().not_null());
    table.check(Expr::col(ActionInvocations::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(ActionInvocations::Trigger).is_in(["tui", "cli"]));
    table.check(Expr::col(ActionInvocations::Outcome).is_in(["ok", "failed"]));
    table.foreign_key(
        ForeignKey::create()
            .from(ActionInvocations::Table, ActionInvocations::SessionId)
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
            .name("idx_action_invocations_ts")
            .table(ActionInvocations::Table)
            .col(ActionInvocations::Ts)
            .to_owned(),
    ]
}
