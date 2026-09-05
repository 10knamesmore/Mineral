//! script_lifecycle 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum ScriptLifecycle {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 事件时间，Unix 毫秒。
    Ts,
    /// 所属会话身份。
    SessionId,
    /// 脚本生命周期事件。
    Event,
    /// 事件详情。
    Detail,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(ScriptLifecycle::Table);
    table.col(
        ColumnDef::new(ScriptLifecycle::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(ScriptLifecycle::Ts).integer().not_null());
    table.col(ColumnDef::new(ScriptLifecycle::SessionId).integer());
    table.col(ColumnDef::new(ScriptLifecycle::Event).text().not_null());
    table.col(ColumnDef::new(ScriptLifecycle::Detail).text());
    table.check(Expr::col(ScriptLifecycle::Event).is_in([
        "load",
        "reload_ok",
        "reload_fail",
        "callback_error",
        "watchdog_abort",
        "config_warning",
    ]));
    table.foreign_key(
        ForeignKey::create()
            .from(ScriptLifecycle::Table, ScriptLifecycle::SessionId)
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
            .name("idx_script_lifecycle_ts")
            .table(ScriptLifecycle::Table)
            .col(ScriptLifecycle::Ts)
            .to_owned(),
    ]
}
