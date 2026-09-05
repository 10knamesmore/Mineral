//! hook_fires 的持久化实体。

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
    pub id: i64,

    /// 事件时间，Unix 毫秒。
    pub ts: i64,

    /// 所属会话身份。
    pub session_id: Option<i64>,

    /// 来源稳定名。
    pub ns: Option<String>,

    /// 来源内歌曲身份。
    pub song_value: Option<String>,

    /// hook 类型。
    pub hook: crate::HookKind,

    /// hook 执行阶段。
    pub stage: crate::HookStage,

    /// hook 裁决。
    pub decision: crate::HookDecision,

    /// 失败放行原因。
    pub fail_open: Option<crate::FailOpen>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
