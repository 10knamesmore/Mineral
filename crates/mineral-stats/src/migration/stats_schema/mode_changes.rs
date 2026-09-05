//! mode_changes 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum ModeChanges {
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
    /// 变更前播放模式。
    FromMode,
    /// 变更后播放模式。
    ToMode,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(ModeChanges::Table);
    table.col(
        ColumnDef::new(ModeChanges::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(ModeChanges::Ts).integer().not_null());
    table.col(ColumnDef::new(ModeChanges::SessionId).integer());
    table.col(ColumnDef::new(ModeChanges::Actor).text().not_null());
    table.col(ColumnDef::new(ModeChanges::FromMode).text().not_null());
    table.col(ColumnDef::new(ModeChanges::ToMode).text().not_null());
    table.check(Expr::col(ModeChanges::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(ModeChanges::FromMode).is_in([
        "sequential",
        "shuffle",
        "repeat_all",
        "repeat_one",
    ]));
    table.check(Expr::col(ModeChanges::ToMode).is_in([
        "sequential",
        "shuffle",
        "repeat_all",
        "repeat_one",
    ]));
    table.foreign_key(
        ForeignKey::create()
            .from(ModeChanges::Table, ModeChanges::SessionId)
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
            .name("idx_mode_changes_ts")
            .table(ModeChanges::Table)
            .col(ModeChanges::Ts)
            .to_owned(),
    ]
}
