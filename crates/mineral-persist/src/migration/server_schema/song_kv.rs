//! song_kv 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, Iden, Index, IndexCreateStatement, Table,
    TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SongKv {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌曲身份。
    SongValue,
    /// 记录键。
    Key,
    /// 值的类型标签。
    Vtype,
    /// 整数或布尔值。
    IntVal,
    /// 实数值。
    RealVal,
    /// 文本值。
    TextVal,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SongKv::Table);
    table.col(ColumnDef::new(SongKv::Namespace).text().not_null());
    table.col(ColumnDef::new(SongKv::SongValue).text().not_null());
    table.col(ColumnDef::new(SongKv::Key).text().not_null());
    table.col(ColumnDef::new(SongKv::Vtype).text().not_null());
    table.col(ColumnDef::new(SongKv::IntVal).integer());
    table.col(ColumnDef::new(SongKv::RealVal).double());
    table.col(ColumnDef::new(SongKv::TextVal).text());
    table.primary_key(
        Index::create()
            .col(SongKv::Namespace)
            .col(SongKv::SongValue)
            .col(SongKv::Key),
    );
    table.check(
        Expr::col(SongKv::Vtype)
            .is_in(["int", "bool"])
            .and(Expr::col(SongKv::IntVal).is_null().not())
            .and(Expr::col(SongKv::RealVal).is_null())
            .and(Expr::col(SongKv::TextVal).is_null())
            .or(Expr::col(SongKv::Vtype)
                .eq(Expr::value("real"))
                .and(Expr::col(SongKv::RealVal).is_null().not())
                .and(Expr::col(SongKv::IntVal).is_null())
                .and(Expr::col(SongKv::TextVal).is_null()))
            .or(Expr::col(SongKv::Vtype)
                .eq(Expr::value("text"))
                .and(Expr::col(SongKv::TextVal).is_null().not())
                .and(Expr::col(SongKv::IntVal).is_null())
                .and(Expr::col(SongKv::RealVal).is_null())),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
