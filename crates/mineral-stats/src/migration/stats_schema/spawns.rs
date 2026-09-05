//! spawns 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Spawns {
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
    /// 程序名称。
    Program,
    /// 操作结果。
    Outcome,
    /// 进程退出码。
    ExitCode,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Spawns::Table);
    table.col(
        ColumnDef::new(Spawns::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(Spawns::Ts).integer().not_null());
    table.col(ColumnDef::new(Spawns::SessionId).integer());
    table.col(ColumnDef::new(Spawns::Actor).text().not_null());
    table.col(ColumnDef::new(Spawns::Program).text().not_null());
    table.col(ColumnDef::new(Spawns::Outcome).text().not_null());
    table.col(ColumnDef::new(Spawns::ExitCode).integer());
    table.check(Expr::col(Spawns::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(Spawns::Outcome).is_in(["exited", "killed", "spawn_failed"]));
    table.foreign_key(
        ForeignKey::create()
            .from(Spawns::Table, Spawns::SessionId)
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
            .name("idx_spawns_ts")
            .table(Spawns::Table)
            .col(Spawns::Ts)
            .to_owned(),
    ]
}
