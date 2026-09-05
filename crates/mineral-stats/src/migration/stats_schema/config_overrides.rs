//! config_overrides 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum ConfigOverrides {
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
    /// 操作涉及的路径。
    Path,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(ConfigOverrides::Table);
    table.col(
        ColumnDef::new(ConfigOverrides::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(ConfigOverrides::Ts).integer().not_null());
    table.col(ColumnDef::new(ConfigOverrides::SessionId).integer());
    table.col(ColumnDef::new(ConfigOverrides::Actor).text().not_null());
    table.col(ColumnDef::new(ConfigOverrides::Path).text().not_null());
    table.check(Expr::col(ConfigOverrides::Actor).is_in(["user", "script", "system", "cli"]));
    table.foreign_key(
        ForeignKey::create()
            .from(ConfigOverrides::Table, ConfigOverrides::SessionId)
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
            .name("idx_config_overrides_ts")
            .table(ConfigOverrides::Table)
            .col(ConfigOverrides::Ts)
            .to_owned(),
    ]
}
