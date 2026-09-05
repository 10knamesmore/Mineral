//! fetches 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Fetches {
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
    /// 获取目标类型。
    FetchKind,
    /// 来源标识。
    Source,
    /// 目标身份。
    TargetRef,
    /// 触发方式。
    Trigger,
    /// 操作结果。
    Outcome,
    /// 操作耗时，单位毫秒。
    LatencyMs,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Fetches::Table);
    table.col(
        ColumnDef::new(Fetches::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(Fetches::Ts).integer().not_null());
    table.col(ColumnDef::new(Fetches::SessionId).integer());
    table.col(ColumnDef::new(Fetches::Actor).text().not_null());
    table.col(ColumnDef::new(Fetches::FetchKind).text().not_null());
    table.col(ColumnDef::new(Fetches::Source).text().not_null());
    table.col(ColumnDef::new(Fetches::TargetRef).text());
    table.col(ColumnDef::new(Fetches::Trigger).text().not_null());
    table.col(ColumnDef::new(Fetches::Outcome).text().not_null());
    table.col(ColumnDef::new(Fetches::LatencyMs).integer().not_null());
    table.check(Expr::col(Fetches::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(Fetches::FetchKind).is_in([
        "my_playlists",
        "playlist_detail",
        "song_url",
        "lyrics",
        "remote_play_count",
        "search",
        "artist_detail",
        "artist_albums",
        "album_detail",
    ]));
    table.check(Expr::col(Fetches::Trigger).is_in(["user", "system"]));
    table.check(Expr::col(Fetches::Outcome).is_in(["ok", "failed", "cancelled"]));
    table.foreign_key(
        ForeignKey::create()
            .from(Fetches::Table, Fetches::SessionId)
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
            .name("idx_fetches_ts")
            .table(Fetches::Table)
            .col(Fetches::Ts)
            .to_owned(),
    ]
}
