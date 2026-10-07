//! m20261007_album_cache 建表时的 artists 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一次专辑详情抓取中的持久记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "album_cache_artists")]
pub struct Model {
    /// 专辑所属来源。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub namespace: String,

    /// 来源内专辑身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub album_id: String,

    /// 专辑艺人的原始顺序。
    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,

    /// 同一来源的艺人身份。
    #[sea_orm(column_type = "Text")]
    pub artist_id: String,

    /// 抓取时的艺人名称。
    #[sea_orm(column_type = "Text")]
    pub name: String,
}

/// 本次迁移创建的数据库关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// 随专辑删除的艺人记录。
    #[sea_orm(
        belongs_to = "super::albums::Entity",
        from = "(Column::Namespace, Column::AlbumId)",
        to = "(super::albums::Column::Namespace, super::albums::Column::AlbumId)",
        on_delete = "Cascade"
    )]
    Album,
}

impl ActiveModelBehavior for ActiveModel {}
