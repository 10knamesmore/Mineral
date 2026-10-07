//! m20260906_stats 建表时的 love_changes 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "love_changes")]
pub struct Model {
    /// 记录身份。
    #[sea_orm(primary_key)]
    #[sea_orm(column_type = "Integer", nullable)]
    pub id: i64,

    /// 事件时间，Unix 毫秒。
    #[sea_orm(column_type = "Integer")]
    pub ts: i64,

    /// 所属会话身份。
    #[sea_orm(column_type = "Integer")]
    pub session_id: Option<i64>,

    /// 行为发起方。
    #[sea_orm(column_type = "Text")]
    pub actor: String,

    /// 来源稳定名。
    #[sea_orm(column_type = "Text")]
    pub ns: String,

    /// 来源内歌曲身份。
    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    /// 变更后的收藏状态。
    #[sea_orm(column_type = "Integer")]
    pub loved: i64,

    /// 收藏操作来源。
    #[sea_orm(column_type = "Text")]
    pub origin: String,

    /// 远端同步结果。
    #[sea_orm(column_type = "Text")]
    pub remote_mirror: Option<String>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// 事件所属会话。
    #[sea_orm(
        belongs_to = "super::sessions::Entity",
        from = "Column::SessionId",
        to = "super::sessions::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    Session,
}

impl ActiveModelBehavior for ActiveModel {}
