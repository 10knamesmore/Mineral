//! m20261007_album_cache 建表时的 albums 实体快照。

use sea_orm::prelude::DateTimeUtc;
use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一次专辑详情抓取中的持久记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "album_cache")]
pub struct Model {
    /// 专辑所属来源。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub namespace: String,

    /// 来源内专辑身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub album_id: String,

    /// 专辑名称。
    #[sea_orm(column_type = "Text")]
    pub name: String,

    /// 专辑简介。
    #[sea_orm(column_type = "Text")]
    pub description: String,

    /// 发行方。
    #[sea_orm(column_type = "Text")]
    pub company: Option<String>,

    /// 发行时间，Unix 毫秒。
    pub publish_time_ms: i64,

    /// 来源给出的曲目数，未知时为空。
    pub track_count: Option<i64>,

    /// 封面位置。
    #[sea_orm(column_type = "Text")]
    pub cover_url: Option<String>,

    /// channel 确定的 UTC 过期时间，读取不续期。
    pub expired_at: DateTimeUtc,
}

/// 数据库关系约束由缓存迁移声明。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
