//! songs 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "songs")]
pub struct Model {
    /// 来源稳定名。
    #[sea_orm(primary_key, auto_increment = false)]
    pub ns: String,

    /// 来源内歌曲身份。
    #[sea_orm(primary_key, auto_increment = false)]
    pub song_value: String,

    /// 展示名称。
    pub name: String,

    /// 别名或译名。
    pub alias: Option<String>,

    /// 来源内专辑身份。
    pub album_id: Option<String>,

    /// 专辑名称。
    pub album_name: Option<String>,

    /// 已知时长，单位毫秒。
    pub duration_ms: Option<i64>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
