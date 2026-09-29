//! session_state 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "session_state")]
pub struct Model {
    /// 记录身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Integer", nullable)]
    pub id: i64,

    /// 当前歌曲来源。
    #[sea_orm(column_type = "Text")]
    pub cur_namespace: Option<String>,

    /// 当前歌曲身份。
    #[sea_orm(column_type = "Text")]
    pub cur_song_value: Option<String>,

    /// 播放进度，单位毫秒。
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub position_ms: i64,

    /// 播放模式。
    #[sea_orm(column_type = "Text")]
    pub play_mode: String,

    /// 音量比例。
    pub volume: f64,

    /// 最近更新时间，Unix 毫秒。
    #[sea_orm(column_type = "Integer")]
    pub updated_at: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
