//! playlist_ops 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playlist_ops")]
pub struct Model {
    /// 记录身份。
    #[sea_orm(primary_key)]
    pub id: i64,

    /// 事件时间，Unix 毫秒。
    pub ts: i64,

    /// 所属会话身份。
    pub session_id: Option<i64>,

    /// 行为发起方。
    pub actor: crate::Actor,

    /// 执行的操作。
    pub op: crate::PlaylistOpKind,

    /// 操作目标歌单身份。
    pub playlist_ref: String,

    /// 来源稳定名。
    pub ns: Option<String>,

    /// 来源内歌曲身份。
    pub song_value: Option<String>,

    /// 操作涉及的歌曲数。
    pub song_count: i64,

    /// 操作结果。
    pub outcome: crate::OpOutcome,

    /// 错误分类。
    pub error_kind: Option<crate::PlaylistError>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
