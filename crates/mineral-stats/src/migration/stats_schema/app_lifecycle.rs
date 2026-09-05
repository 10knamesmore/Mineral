//! app_lifecycle 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum AppLifecycle {
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
    /// 生命周期所属组件。
    Who,
    /// 生命周期阶段。
    Phase,
    /// 音频后端。
    AudioBackend,
    /// 是否恢复已有会话。
    SessionRestored,
    /// 客户端版本。
    ClientVersion,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(AppLifecycle::Table);
    table.col(
        ColumnDef::new(AppLifecycle::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(AppLifecycle::Ts).integer().not_null());
    table.col(ColumnDef::new(AppLifecycle::SessionId).integer());
    table.col(ColumnDef::new(AppLifecycle::Actor).text().not_null());
    table.col(ColumnDef::new(AppLifecycle::Who).text().not_null());
    table.col(ColumnDef::new(AppLifecycle::Phase).text().not_null());
    table.col(ColumnDef::new(AppLifecycle::AudioBackend).text());
    table.col(ColumnDef::new(AppLifecycle::SessionRestored).integer());
    table.col(ColumnDef::new(AppLifecycle::ClientVersion).text());
    table.check(Expr::col(AppLifecycle::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(AppLifecycle::Who).is_in(["daemon", "client"]));
    table.check(Expr::col(AppLifecycle::Phase).is_in(["start", "stop"]));
    table.check(Expr::col(AppLifecycle::AudioBackend).is_in(["device", "null"]));
    table.foreign_key(
        ForeignKey::create()
            .from(AppLifecycle::Table, AppLifecycle::SessionId)
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
            .name("idx_app_lifecycle_ts")
            .table(AppLifecycle::Table)
            .col(AppLifecycle::Ts)
            .to_owned(),
    ]
}
