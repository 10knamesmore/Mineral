use sea_orm::prelude::DateTimeUtc;
use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "album_cache")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub namespace: String,

    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub album_id: String,

    #[sea_orm(column_type = "Text")]
    pub name: String,

    #[sea_orm(column_type = "Text")]
    pub description: String,

    #[sea_orm(column_type = "Text")]
    pub company: Option<String>,

    /// 发行时间，Unix 毫秒；未知时为空。
    pub publish_time_ms: Option<i64>,

    pub track_count: Option<i64>,

    #[sea_orm(column_type = "Text")]
    pub cover_url: Option<String>,

    pub expired_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
