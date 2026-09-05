//! session_queue 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "session_queue")]
pub struct Model {
    /// 原始排列位置。
    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,

    /// 来源稳定名。
    pub namespace: String,

    /// 来源内歌曲身份。
    pub song_value: String,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
