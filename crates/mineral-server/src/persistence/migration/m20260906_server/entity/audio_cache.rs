//! m20260906_server 建表时的 audio_cache 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "audio_cache")]
pub struct Model {
    /// 记录键。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text", nullable)]
    pub key: String,

    /// 相对缓存根目录的路径。
    #[sea_orm(column_type = "Text")]
    pub relpath: String,

    /// 文件字节数。
    #[sea_orm(column_type = "Integer")]
    pub bytes: i64,

    /// 最近访问的逻辑时钟。
    #[sea_orm(column_type = "Integer")]
    pub last_access: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
