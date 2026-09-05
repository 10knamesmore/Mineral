//! song_envelope 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "song_envelope")]
pub struct Model {
    /// 来源稳定名。
    #[sea_orm(primary_key, auto_increment = false)]
    pub namespace: String,

    /// 来源内歌曲身份。
    #[sea_orm(primary_key, auto_increment = false)]
    pub song_value: String,

    /// 数据编码版本。
    pub version: i64,

    /// 包络点编码。
    pub points: Vec<u8>,

    /// 最近更新时间，Unix 毫秒。
    pub updated_at: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
