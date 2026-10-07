//! m20260906_server 建表时的 song_artists 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "song_artists")]
pub struct Model {
    /// 来源稳定名。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub namespace: String,

    /// 来源内歌曲身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    /// 原始排列位置。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Integer")]
    pub position: i64,

    /// 来源内艺人身份。
    #[sea_orm(column_type = "Text")]
    pub artist_id: String,

    /// 艺人名称。
    #[sea_orm(column_type = "Text")]
    pub artist_name: String,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// 随歌曲删除的艺人记录。
    #[sea_orm(
        belongs_to = "super::song_meta::Entity",
        from = "(Column::Namespace, Column::SongValue)",
        to = "(super::song_meta::Column::Namespace, super::song_meta::Column::SongValue)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Song,
}

impl ActiveModelBehavior for ActiveModel {}
