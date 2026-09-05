//! playlist_entries 的持久化实体。

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
    pub playlist_namespace: String,

    /// 来源内歌单身份。
    #[sea_orm(primary_key, auto_increment = false)]
    pub playlist_value: String,

    /// 歌单条目的原始位置。
    #[sea_orm(primary_key, auto_increment = false)]
    pub collection_index: i64,

    /// 歌曲来源。
    pub song_namespace: String,

    /// 来源内歌曲身份。
    pub song_value: String,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
