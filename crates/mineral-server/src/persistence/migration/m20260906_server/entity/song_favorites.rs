//! m20260906_server 建表时的 song_favorites 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "song_favorites")]
pub struct Model {
    /// 来源稳定名。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub namespace: String,

    /// 来源内歌曲身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    /// 加入收藏的排序时间，以 Unix 毫秒为基准；新操作至少比现有最大值多 1，导入批次内共用。
    #[sea_orm(column_type = "Integer")]
    pub entered_at: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
