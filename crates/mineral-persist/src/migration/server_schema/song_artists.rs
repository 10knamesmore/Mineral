//! song_artists 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, ForeignKey, ForeignKeyAction, Iden, Index, IndexCreateStatement, Table,
    TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum SongArtists {
    /// 数据表。
    Table,
    /// 来源稳定名。
    Namespace,
    /// 来源内歌曲身份。
    SongValue,
    /// 原始排列位置。
    Position,
    /// 来源内艺人身份。
    ArtistId,
    /// 艺人名称。
    ArtistName,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(SongArtists::Table);
    table.col(ColumnDef::new(SongArtists::Namespace).text().not_null());
    table.col(ColumnDef::new(SongArtists::SongValue).text().not_null());
    table.col(ColumnDef::new(SongArtists::Position).integer().not_null());
    table.col(ColumnDef::new(SongArtists::ArtistId).text().not_null());
    table.col(ColumnDef::new(SongArtists::ArtistName).text().not_null());
    table.primary_key(
        Index::create()
            .col(SongArtists::Namespace)
            .col(SongArtists::SongValue)
            .col(SongArtists::Position),
    );
    table.foreign_key(
        ForeignKey::create()
            .from(
                SongArtists::Table,
                (SongArtists::Namespace, SongArtists::SongValue),
            )
            .to(
                super::song_meta::SongMeta::Table,
                (
                    super::song_meta::SongMeta::Namespace,
                    super::song_meta::SongMeta::SongValue,
                ),
            )
            .on_update(ForeignKeyAction::NoAction)
            .on_delete(ForeignKeyAction::Cascade),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![]
}
