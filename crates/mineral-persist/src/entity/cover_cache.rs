//! cover_cache 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "cover_cache")]
pub struct Model {
    /// 记录键。
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,

    /// 相对缓存根目录的路径。
    pub relpath: String,

    /// 文件字节数。
    pub bytes: i64,

    /// 最近访问的逻辑时钟。
    pub last_access: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
