//! session_state 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, Iden, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SessionState {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 当前歌曲来源。
    CurNamespace,
    /// 当前歌曲身份。
    CurSongValue,
    /// 播放进度，单位毫秒。
    PositionMs,
    /// 播放模式。
    PlayMode,
    /// 音量比例。
    Volume,
    /// 最近更新时间，Unix 毫秒。
    UpdatedAt,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SessionState::Table);
    table.col(ColumnDef::new(SessionState::Id).integer().primary_key());
    table.col(ColumnDef::new(SessionState::CurNamespace).text());
    table.col(ColumnDef::new(SessionState::CurSongValue).text());
    table.col(
        ColumnDef::new(SessionState::PositionMs)
            .integer()
            .not_null()
            .default(0i64),
    );
    table.col(ColumnDef::new(SessionState::PlayMode).text().not_null());
    table.col(ColumnDef::new(SessionState::Volume).double().not_null());
    table.col(ColumnDef::new(SessionState::UpdatedAt).integer().not_null());
    table.check(
        Expr::col(SessionState::CurNamespace)
            .is_null()
            .eq(Expr::col(SessionState::CurSongValue).is_null()),
    );
    table.check(Expr::col(SessionState::Id).eq(Expr::value(0i64)));
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
