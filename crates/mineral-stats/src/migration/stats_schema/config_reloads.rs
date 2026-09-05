//! config_reloads 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, ForeignKey, ForeignKeyAction, Iden, Index, IndexCreateStatement, Table,
    TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum ConfigReloads {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 事件时间，Unix 毫秒。
    Ts,
    /// 所属会话身份。
    SessionId,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(ConfigReloads::Table);
    table.col(
        ColumnDef::new(ConfigReloads::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(ConfigReloads::Ts).integer().not_null());
    table.col(ColumnDef::new(ConfigReloads::SessionId).integer());
    table.foreign_key(
        ForeignKey::create()
            .from(ConfigReloads::Table, ConfigReloads::SessionId)
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
            .name("idx_config_reloads_ts")
            .table(ConfigReloads::Table)
            .col(ConfigReloads::Ts)
            .to_owned(),
    ]
}
