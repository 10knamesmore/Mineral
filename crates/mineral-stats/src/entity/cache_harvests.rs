//! cache_harvests 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "cache_harvests")]
pub struct Model {
    /// 记录身份。
    #[sea_orm(primary_key)]
    pub id: i64,

    /// 事件时间，Unix 毫秒。
    pub ts: i64,

    /// 所属会话身份。
    pub session_id: Option<i64>,

    /// 来源稳定名。
    pub ns: String,

    /// 来源内歌曲身份。
    pub song_value: String,

    /// 音质标识。
    pub quality: String,

    /// 音频格式。
    pub format: String,

    /// 操作结果。
    pub outcome: crate::CacheHarvestOutcome,

    /// 文件字节数。
    pub bytes: Option<i64>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
