//! m20260906_server 建表时的 song_envelope 实体快照。

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
    #[sea_orm(column_type = "Text")]
    pub namespace: String,

    /// 来源内歌曲身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    /// 数据编码版本。
    #[sea_orm(column_type = "Integer")]
    pub version: i64,

    /// 包络点编码。
    #[sea_orm(column_type = "Binary(1)")]
    pub points: Vec<u8>,

    /// 最近更新时间，Unix 毫秒。
    #[sea_orm(column_type = "Integer")]
    pub updated_at: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
