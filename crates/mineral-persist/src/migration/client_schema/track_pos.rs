//! track_pos 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum TrackPos {
    /// 数据表。
    Table,
    /// 歌单来源。
    PlaylistNamespace,
    /// 来源内歌单身份。
    PlaylistValue,
    /// 歌曲来源。
    SongNamespace,
    /// 来源内歌曲身份。
    SongValue,
    /// 选中条目的原始位置。
    SelIndex,
    /// 选中项在视口内的行位置。
    ScreenRow,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(TrackPos::Table);
    table.col(
        ColumnDef::new(TrackPos::PlaylistNamespace)
            .text()
            .not_null(),
    );
    table.col(ColumnDef::new(TrackPos::PlaylistValue).text().not_null());
    table.col(ColumnDef::new(TrackPos::SongNamespace).text().not_null());
    table.col(ColumnDef::new(TrackPos::SongValue).text().not_null());
    table.col(ColumnDef::new(TrackPos::SelIndex).integer().not_null());
    table.col(ColumnDef::new(TrackPos::ScreenRow).integer().not_null());
    table.primary_key(
        Index::create()
            .col(TrackPos::PlaylistNamespace)
            .col(TrackPos::PlaylistValue),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
