//! m20260906_stats 建表时的 copy_renders 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "copy_renders")]
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

    /// 复制模板位置。
    #[sea_orm(column_type = "Integer")]
    pub template_index: i64,

    /// 复制上下文类型。
    #[sea_orm(column_type = "Text")]
    pub ctx_kind: String,

    /// 目标身份。
    #[sea_orm(column_type = "Text")]
    pub target_ref: Option<String>,

    /// 操作结果。
    #[sea_orm(column_type = "Text")]
    pub outcome: String,
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
