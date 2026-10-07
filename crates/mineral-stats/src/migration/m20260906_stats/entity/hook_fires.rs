//! m20260906_stats 建表时的 hook_fires 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "hook_fires")]
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

    /// 来源稳定名。
    #[sea_orm(column_type = "Text")]
    pub ns: Option<String>,

    /// 来源内歌曲身份。
    #[sea_orm(column_type = "Text")]
    pub song_value: Option<String>,

    /// hook 类型。
    #[sea_orm(column_type = "Text")]
    pub hook: String,

    /// hook 执行阶段。
    #[sea_orm(column_type = "Text")]
    pub stage: String,

    /// hook 裁决。
    #[sea_orm(column_type = "Text")]
    pub decision: String,

    /// 失败放行原因。
    #[sea_orm(column_type = "Text")]
    pub fail_open: Option<String>,
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
