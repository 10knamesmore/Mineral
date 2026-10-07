//! m20260906_client 建表时的 track_pos 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "track_pos")]
pub struct Model {
    /// 歌单来源。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub playlist_namespace: String,

    /// 来源内歌单身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub playlist_value: String,

    /// 歌曲来源。
    #[sea_orm(column_type = "Text")]
    pub song_namespace: String,

    /// 来源内歌曲身份。
    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    /// 选中条目的原始位置。
    #[sea_orm(column_type = "Integer")]
    pub sel_index: i64,

    /// 选中项在视口内的行位置。
    #[sea_orm(column_type = "Integer")]
    pub screen_row: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
