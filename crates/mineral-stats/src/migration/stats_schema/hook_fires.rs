//! hook_fires 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum HookFires {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 事件时间，Unix 毫秒。
    Ts,
    /// 所属会话身份。
    SessionId,
    /// 来源稳定名。
    Ns,
    /// 来源内歌曲身份。
    SongValue,
    /// hook 类型。
    Hook,
    /// hook 执行阶段。
    Stage,
    /// hook 裁决。
    Decision,
    /// 失败放行原因。
    FailOpen,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(HookFires::Table);
    table.col(
        ColumnDef::new(HookFires::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(HookFires::Ts).integer().not_null());
    table.col(ColumnDef::new(HookFires::SessionId).integer());
    table.col(ColumnDef::new(HookFires::Ns).text());
    table.col(ColumnDef::new(HookFires::SongValue).text());
    table.col(ColumnDef::new(HookFires::Hook).text().not_null());
    table.col(ColumnDef::new(HookFires::Stage).text().not_null());
    table.col(ColumnDef::new(HookFires::Decision).text().not_null());
    table.col(ColumnDef::new(HookFires::FailOpen).text());
    table.check(Expr::col(HookFires::Hook).is_in(["before_stream", "before_download"]));
    table.check(Expr::col(HookFires::Stage).is_in(["immediate", "prefetch"]));
    table.check(Expr::col(HookFires::Decision).is_in(["continue", "rewrite", "skip"]));
    table.check(Expr::col(HookFires::FailOpen).is_in(["timeout", "thread_dead", "error"]));
    table.foreign_key(
        ForeignKey::create()
            .from(HookFires::Table, HookFires::SessionId)
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
            .name("idx_hook_fires_ts")
            .table(HookFires::Table)
            .col(HookFires::Ts)
            .to_owned(),
    ]
}
