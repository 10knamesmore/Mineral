//! 每条专辑曲目的有序艺人快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一次专辑详情抓取中的持久记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "album_cache_track_artists")]
pub struct Model {
    /// 专辑所属来源。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub namespace: String,

    /// 来源内专辑身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub album_id: String,

    /// 所属专辑曲目的原始位置。
    #[sea_orm(primary_key, auto_increment = false)]
    pub collection_index: i64,

    /// 歌曲艺人的原始顺序。
    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,

    /// 同一来源的艺人身份。
    #[sea_orm(column_type = "Text")]
    pub artist_id: String,

    /// 抓取时的艺人名称。
    #[sea_orm(column_type = "Text")]
    pub name: String,
}

/// 数据库关系约束由缓存迁移声明。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
