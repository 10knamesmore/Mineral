//! song_stats 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "song_stats")]
pub struct Model {
    /// 来源稳定名。
    #[sea_orm(primary_key, auto_increment = false)]
    pub namespace: String,

    /// 来源内歌曲身份。
    #[sea_orm(primary_key, auto_increment = false)]
    pub song_value: String,

    /// 播放次数。
    pub play_count: i64,

    /// 跳过次数。
    pub skip_count: i64,

    /// 累计收听毫秒数。
    pub total_listen_ms: i64,

    /// 最近播放时间，Unix 毫秒。
    pub last_played_at: Option<i64>,

    /// 用户评分。
    pub rating: Option<i64>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
