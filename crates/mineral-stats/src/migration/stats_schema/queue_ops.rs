//! queue_ops 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum QueueOps {
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
    /// 执行的操作。
    Op,
    /// 来源稳定名。
    Ns,
    /// 来源内歌曲身份。
    SongValue,
    /// 涉及的条目数量。
    Count,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(QueueOps::Table);
    table.col(
        ColumnDef::new(QueueOps::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(QueueOps::Ts).integer().not_null());
    table.col(ColumnDef::new(QueueOps::SessionId).integer());
    table.col(ColumnDef::new(QueueOps::Actor).text().not_null());
    table.col(ColumnDef::new(QueueOps::Op).text().not_null());
    table.col(ColumnDef::new(QueueOps::Ns).text());
    table.col(ColumnDef::new(QueueOps::SongValue).text());
    table.col(ColumnDef::new(QueueOps::Count).integer().not_null());
    table.check(Expr::col(QueueOps::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(QueueOps::Op).is_in([
        "set",
        "insert_next",
        "append",
        "clear",
        "remove",
        "move",
        "clear_above",
        "clear_below",
        "transform",
        "undo",
    ]));
    table.foreign_key(
        ForeignKey::create()
            .from(QueueOps::Table, QueueOps::SessionId)
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
            .name("idx_queue_ops_ts")
            .table(QueueOps::Table)
            .col(QueueOps::Ts)
            .to_owned(),
    ]
}
