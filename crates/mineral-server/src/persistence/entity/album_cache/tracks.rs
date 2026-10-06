//! 专辑曲目关系及抓取时的完整歌曲快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一次专辑详情抓取中的持久记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "album_cache_tracks")]
pub struct Model {
    /// 专辑所属来源。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub namespace: String,

    /// 来源内专辑身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub album_id: String,

    /// 专辑响应中的原始曲目位置。
    #[sea_orm(primary_key, auto_increment = false)]
    pub collection_index: i64,

    /// 同一来源的歌曲身份。
    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    /// 抓取时的歌名。
    #[sea_orm(column_type = "Text")]
    pub name: String,

    /// 歌曲别名。
    #[sea_orm(column_type = "Text")]
    pub alias: Option<String>,

    /// 歌曲自身引用的同一来源专辑身份。
    #[sea_orm(column_type = "Text")]
    pub song_album_id: Option<String>,

    /// 歌曲自身引用的专辑名称，与专辑身份成对存在。
    #[sea_orm(column_type = "Text")]
    pub song_album_name: Option<String>,

    /// 已知时长，毫秒。
    pub duration_ms: Option<i64>,

    /// 歌曲封面位置。
    #[sea_orm(column_type = "Text")]
    pub cover_url: Option<String>,

    /// 来源提供的位置。
    #[sea_orm(column_type = "Text")]
    pub source_url: Option<String>,

    /// 抓取时来源给出的不可用状态。
    pub unavailable: bool,
}

/// 数据库关系约束由缓存迁移声明。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
