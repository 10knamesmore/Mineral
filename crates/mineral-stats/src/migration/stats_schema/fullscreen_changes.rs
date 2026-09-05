//! fullscreen_changes 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum FullscreenChanges {
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
    /// 是否处于全屏。
    Fullscreen,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(FullscreenChanges::Table);
    table.col(
        ColumnDef::new(FullscreenChanges::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(FullscreenChanges::Ts).integer().not_null());
    table.col(ColumnDef::new(FullscreenChanges::SessionId).integer());
    table.col(ColumnDef::new(FullscreenChanges::Actor).text().not_null());
    table.col(
        ColumnDef::new(FullscreenChanges::Fullscreen)
            .integer()
            .not_null(),
    );
    table.check(Expr::col(FullscreenChanges::Actor).is_in(["user", "script", "system", "cli"]));
    table.foreign_key(
        ForeignKey::create()
            .from(FullscreenChanges::Table, FullscreenChanges::SessionId)
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
            .name("idx_fullscreen_changes_ts")
            .table(FullscreenChanges::Table)
            .col(FullscreenChanges::Ts)
            .to_owned(),
    ]
}
