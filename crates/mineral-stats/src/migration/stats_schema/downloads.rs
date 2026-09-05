//! downloads 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Downloads {
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
    /// 音质标识。
    Quality,
    /// 音频格式。
    Format,
    /// 操作结果。
    Outcome,
    /// 下载 hook 执行结果。
    Hooked,
    /// 操作涉及的路径。
    Path,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Downloads::Table);
    table.col(
        ColumnDef::new(Downloads::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(Downloads::Ts).integer().not_null());
    table.col(ColumnDef::new(Downloads::SessionId).integer());
    table.col(ColumnDef::new(Downloads::Actor).text().not_null());
    table.col(ColumnDef::new(Downloads::Ns).text().not_null());
    table.col(ColumnDef::new(Downloads::SongValue).text().not_null());
    table.col(ColumnDef::new(Downloads::Quality).text().not_null());
    table.col(ColumnDef::new(Downloads::Format).text());
    table.col(ColumnDef::new(Downloads::Outcome).text().not_null());
    table.col(ColumnDef::new(Downloads::Hooked).text().not_null());
    table.col(ColumnDef::new(Downloads::Path).text());
    table.check(Expr::col(Downloads::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(Downloads::Outcome).is_in(["downloaded", "skipped", "failed"]));
    table.check(Expr::col(Downloads::Hooked).is_in(["none", "rewrite", "skip"]));
    table.foreign_key(
        ForeignKey::create()
            .from(Downloads::Table, Downloads::SessionId)
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
            .name("idx_downloads_ts")
            .table(Downloads::Table)
            .col(Downloads::Ts)
            .to_owned(),
    ]
}
