//! playlist_cache 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playlist_cache")]
pub struct Model {
    /// 来源稳定名。
    #[sea_orm(primary_key, auto_increment = false)]
    pub namespace: String,

    /// 来源内歌单身份。
    #[sea_orm(primary_key, auto_increment = false)]
    pub playlist_id: String,

    /// 展示名称。
    pub name: Option<String>,

    /// 取回时间，Unix 毫秒。
    pub fetched_at: i64,

    /// 来源提供的曲目更新时间。
    pub track_update_time: Option<i64>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
