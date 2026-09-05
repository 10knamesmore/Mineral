//! song_artists 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Iden, Index, IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SongArtists {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Ns,
    /// 来源内歌曲身份。
    SongValue,
    /// 原始排列位置。
    Position,
    /// 来源内艺人身份。
    ArtistValue,
    /// 艺人名称。
    ArtistName,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SongArtists::Table);
    table.col(ColumnDef::new(SongArtists::Ns).text().not_null());
    table.col(ColumnDef::new(SongArtists::SongValue).text().not_null());
    table.col(ColumnDef::new(SongArtists::Position).integer().not_null());
    table.col(ColumnDef::new(SongArtists::ArtistValue).text().not_null());
    table.col(ColumnDef::new(SongArtists::ArtistName).text().not_null());
    table.primary_key(
        Index::create()
            .col(SongArtists::Ns)
            .col(SongArtists::SongValue)
            .col(SongArtists::Position),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![
        Index::create()
            .name("idx_song_artists_artist")
            .table(SongArtists::Table)
            .col(SongArtists::Ns)
            .col(SongArtists::ArtistValue)
            .to_owned(),
    ]
}
