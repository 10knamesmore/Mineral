//! m20260906_server 建表时的 playlist_entries 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playlist_entries")]
pub struct Model {
    /// 歌单来源。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub playlist_namespace: String,

    /// 来源内歌单身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub playlist_value: String,

    /// 歌单条目的原始位置。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Integer")]
    pub collection_index: i64,

    /// 歌曲来源。
    #[sea_orm(column_type = "Text")]
    pub song_namespace: String,

    /// 来源内歌曲身份。
    #[sea_orm(column_type = "Text")]
    pub song_value: String,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// 随歌单删除的条目。
    #[sea_orm(
        belongs_to = "super::playlist_cache::Entity",
        from = "(Column::PlaylistNamespace, Column::PlaylistValue)",
        to = "(super::playlist_cache::Column::Namespace, super::playlist_cache::Column::PlaylistId)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Playlist,
}

impl ActiveModelBehavior for ActiveModel {}
