//! m20260906_stats 建表时的 app_lifecycle 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "app_lifecycle")]
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

    /// 生命周期所属组件。
    #[sea_orm(column_type = "Text")]
    pub who: String,

    /// 生命周期阶段。
    #[sea_orm(column_type = "Text")]
    pub phase: String,

    /// 音频后端。
    #[sea_orm(column_type = "Text")]
    pub audio_backend: Option<String>,

    /// 是否恢复已有会话。
    #[sea_orm(column_type = "Integer")]
    pub session_restored: Option<i64>,

    /// 客户端版本。
    #[sea_orm(column_type = "Text")]
    pub client_version: Option<String>,
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
