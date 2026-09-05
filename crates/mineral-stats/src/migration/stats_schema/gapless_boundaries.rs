//! gapless_boundaries 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum GaplessBoundaries {
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
    /// 无缝衔接结果。
    Result,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(GaplessBoundaries::Table);
    table.col(
        ColumnDef::new(GaplessBoundaries::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(GaplessBoundaries::Ts).integer().not_null());
    table.col(ColumnDef::new(GaplessBoundaries::SessionId).integer());
    table.col(ColumnDef::new(GaplessBoundaries::Ns).text().not_null());
    table.col(
        ColumnDef::new(GaplessBoundaries::SongValue)
            .text()
            .not_null(),
    );
    table.col(ColumnDef::new(GaplessBoundaries::Result).text().not_null());
    table.check(Expr::col(GaplessBoundaries::Result).is_in(["adopt", "fallback"]));
    table.foreign_key(
        ForeignKey::create()
            .from(GaplessBoundaries::Table, GaplessBoundaries::SessionId)
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
            .name("idx_gapless_boundaries_ts")
            .table(GaplessBoundaries::Table)
            .col(GaplessBoundaries::Ts)
            .to_owned(),
    ]
}
