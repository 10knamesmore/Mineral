//! love_changes 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum LoveChanges {
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
    /// 变更后的收藏状态。
    Loved,
    /// 收藏操作来源。
    Origin,
    /// 远端同步结果。
    RemoteMirror,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(LoveChanges::Table);
    table.col(
        ColumnDef::new(LoveChanges::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(LoveChanges::Ts).integer().not_null());
    table.col(ColumnDef::new(LoveChanges::SessionId).integer());
    table.col(ColumnDef::new(LoveChanges::Actor).text().not_null());
    table.col(ColumnDef::new(LoveChanges::Ns).text().not_null());
    table.col(ColumnDef::new(LoveChanges::SongValue).text().not_null());
    table.col(ColumnDef::new(LoveChanges::Loved).integer().not_null());
    table.col(ColumnDef::new(LoveChanges::Origin).text().not_null());
    table.col(ColumnDef::new(LoveChanges::RemoteMirror).text());
    table.check(Expr::col(LoveChanges::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(LoveChanges::Origin).is_in(["user", "import"]));
    table.check(Expr::col(LoveChanges::RemoteMirror).is_in(["ok", "not_supported", "failed"]));
    table.foreign_key(
        ForeignKey::create()
            .from(LoveChanges::Table, LoveChanges::SessionId)
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
            .name("idx_love_changes_ts")
            .table(LoveChanges::Table)
            .col(LoveChanges::Ts)
            .to_owned(),
    ]
}
