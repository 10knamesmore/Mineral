//! stream_resolutions 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum StreamResolutions {
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
    /// 请求的音质。
    QualityRequested,
    /// 操作结果。
    Outcome,
    /// 是否用于预取。
    ForPrefetch,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(StreamResolutions::Table);
    table.col(
        ColumnDef::new(StreamResolutions::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(StreamResolutions::Ts).integer().not_null());
    table.col(ColumnDef::new(StreamResolutions::SessionId).integer());
    table.col(ColumnDef::new(StreamResolutions::Ns).text().not_null());
    table.col(
        ColumnDef::new(StreamResolutions::SongValue)
            .text()
            .not_null(),
    );
    table.col(
        ColumnDef::new(StreamResolutions::QualityRequested)
            .text()
            .not_null(),
    );
    table.col(ColumnDef::new(StreamResolutions::Outcome).text().not_null());
    table.col(
        ColumnDef::new(StreamResolutions::ForPrefetch)
            .integer()
            .not_null(),
    );
    table.check(Expr::col(StreamResolutions::Outcome).is_in(["ok", "empty", "error"]));
    table.foreign_key(
        ForeignKey::create()
            .from(StreamResolutions::Table, StreamResolutions::SessionId)
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
            .name("idx_stream_resolutions_ts")
            .table(StreamResolutions::Table)
            .col(StreamResolutions::Ts)
            .to_owned(),
    ]
}
