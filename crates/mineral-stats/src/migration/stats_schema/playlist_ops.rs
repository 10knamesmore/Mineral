//! playlist_ops 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum PlaylistOps {
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
    /// 操作目标歌单身份。
    PlaylistRef,
    /// 来源稳定名。
    Ns,
    /// 来源内歌曲身份。
    SongValue,
    /// 操作涉及的歌曲数。
    SongCount,
    /// 操作结果。
    Outcome,
    /// 错误分类。
    ErrorKind,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(PlaylistOps::Table);
    table.col(
        ColumnDef::new(PlaylistOps::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(PlaylistOps::Ts).integer().not_null());
    table.col(ColumnDef::new(PlaylistOps::SessionId).integer());
    table.col(ColumnDef::new(PlaylistOps::Actor).text().not_null());
    table.col(ColumnDef::new(PlaylistOps::Op).text().not_null());
    table.col(ColumnDef::new(PlaylistOps::PlaylistRef).text().not_null());
    table.col(ColumnDef::new(PlaylistOps::Ns).text());
    table.col(ColumnDef::new(PlaylistOps::SongValue).text());
    table.col(ColumnDef::new(PlaylistOps::SongCount).integer().not_null());
    table.col(ColumnDef::new(PlaylistOps::Outcome).text().not_null());
    table.col(ColumnDef::new(PlaylistOps::ErrorKind).text());
    table.check(Expr::col(PlaylistOps::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(PlaylistOps::Op).is_in([
        "create",
        "delete",
        "add",
        "remove",
        "rename",
        "set_description",
    ]));
    table.check(Expr::col(PlaylistOps::Outcome).is_in(["ok", "failed"]));
    table.check(Expr::col(PlaylistOps::ErrorKind).is_in([
        "auth_required",
        "rate_limited",
        "not_supported",
        "api",
        "other",
    ]));
    table.foreign_key(
        ForeignKey::create()
            .from(PlaylistOps::Table, PlaylistOps::SessionId)
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
            .name("idx_playlist_ops_ts")
            .table(PlaylistOps::Table)
            .col(PlaylistOps::Ts)
            .to_owned(),
    ]
}
