//! 自建歌单的持久记录。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条持久记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "user_playlists")]
pub struct Model {
    /// Mineral 来源内的 UUID。
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,

    /// 用户输入的名称，允许同名。
    pub name: String,

    /// 完整保存的曲目数。
    pub track_count: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
