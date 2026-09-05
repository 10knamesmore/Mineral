//! pauses 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Pauses {
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
    /// 来源稳定名。
    Ns,
    /// 来源内歌曲身份。
    SongValue,
    /// 操作发生时的进度，单位毫秒。
    AtMs,
    /// 暂停或继续动作。
    Action,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Pauses::Table);
    table.col(
        ColumnDef::new(Pauses::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(Pauses::Ts).integer().not_null());
    table.col(ColumnDef::new(Pauses::SessionId).integer());
    table.col(ColumnDef::new(Pauses::Actor).text().not_null());
    table.col(ColumnDef::new(Pauses::Ns).text().not_null());
    table.col(ColumnDef::new(Pauses::SongValue).text().not_null());
    table.col(ColumnDef::new(Pauses::AtMs).integer().not_null());
    table.col(ColumnDef::new(Pauses::Action).text().not_null());
    table.check(Expr::col(Pauses::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(Pauses::Action).is_in(["pause", "resume"]));
    table.foreign_key(
        ForeignKey::create()
            .from(Pauses::Table, Pauses::SessionId)
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
            .name("idx_pauses_ts")
            .table(Pauses::Table)
            .col(Pauses::Ts)
            .to_owned(),
    ]
}
