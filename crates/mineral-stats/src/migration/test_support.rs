//! 迁移验证使用的数据库结构读取。

use sea_orm::sea_query::{self, Expr, ExprTrait, Iden, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityName};

/// SQLite 的结构目录及其查询列。
#[derive(Iden)]
enum SqliteMaster {
    /// 结构目录。
    Table,
    /// 对象类型。
    Type,
    /// 对象名称。
    Name,
}

/// 列出业务表，不含 SQLite 内部表与迁移账本。
pub(crate) async fn table_names(db: &DatabaseConnection) -> color_eyre::Result<Vec<String>> {
    let query = Query::select()
        .column(SqliteMaster::Name)
        .from(SqliteMaster::Table)
        .and_where(Expr::col(SqliteMaster::Type).eq("table"))
        .and_where(Expr::col(SqliteMaster::Name).not_like("sqlite_%"))
        .and_where(
            Expr::col(SqliteMaster::Name)
                .ne(sea_orm_migration::seaql_migrations::Entity.table_name()),
        )
        .to_owned();
    db.query_all(&query)
        .await?
        .into_iter()
        .map(|row| {
            row.try_get_by_index(/*index*/ 0).map_err(Into::into)
        })
        .collect()
}
