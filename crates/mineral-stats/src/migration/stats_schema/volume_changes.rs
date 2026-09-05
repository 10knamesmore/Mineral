//! volume_changes 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum VolumeChanges {
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
    /// 变更前音量百分比。
    FromPct,
    /// 变更后音量百分比。
    ToPct,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(VolumeChanges::Table);
    table.col(
        ColumnDef::new(VolumeChanges::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(VolumeChanges::Ts).integer().not_null());
    table.col(ColumnDef::new(VolumeChanges::SessionId).integer());
    table.col(ColumnDef::new(VolumeChanges::Actor).text().not_null());
    table.col(ColumnDef::new(VolumeChanges::FromPct).integer().not_null());
    table.col(ColumnDef::new(VolumeChanges::ToPct).integer().not_null());
    table.check(Expr::col(VolumeChanges::Actor).is_in(["user", "script", "system", "cli"]));
    table.foreign_key(
        ForeignKey::create()
            .from(VolumeChanges::Table, VolumeChanges::SessionId)
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
            .name("idx_volume_changes_ts")
            .table(VolumeChanges::Table)
            .col(VolumeChanges::Ts)
            .to_owned(),
    ]
}
