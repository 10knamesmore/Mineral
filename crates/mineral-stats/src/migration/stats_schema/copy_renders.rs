//! copy_renders 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum CopyRenders {
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
    /// 复制模板位置。
    TemplateIndex,
    /// 复制上下文类型。
    CtxKind,
    /// 目标身份。
    TargetRef,
    /// 操作结果。
    Outcome,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(CopyRenders::Table);
    table.col(
        ColumnDef::new(CopyRenders::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(CopyRenders::Ts).integer().not_null());
    table.col(ColumnDef::new(CopyRenders::SessionId).integer());
    table.col(ColumnDef::new(CopyRenders::Actor).text().not_null());
    table.col(
        ColumnDef::new(CopyRenders::TemplateIndex)
            .integer()
            .not_null(),
    );
    table.col(ColumnDef::new(CopyRenders::CtxKind).text().not_null());
    table.col(ColumnDef::new(CopyRenders::TargetRef).text());
    table.col(ColumnDef::new(CopyRenders::Outcome).text().not_null());
    table.check(Expr::col(CopyRenders::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(CopyRenders::CtxKind).is_in(["song", "playlist", "album", "artist"]));
    table.check(Expr::col(CopyRenders::Outcome).is_in(["ok", "failed"]));
    table.foreign_key(
        ForeignKey::create()
            .from(CopyRenders::Table, CopyRenders::SessionId)
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
            .name("idx_copy_renders_ts")
            .table(CopyRenders::Table)
            .col(CopyRenders::Ts)
            .to_owned(),
    ]
}
