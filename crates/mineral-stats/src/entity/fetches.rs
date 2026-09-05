//! fetches 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "fetches")]
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

    /// 获取目标类型。
    pub fetch_kind: crate::FetchKind,

    /// 来源标识。
    pub source: String,

    /// 目标身份。
    pub target_ref: Option<String>,

    /// 触发方式。
    pub trigger: crate::FetchTrigger,

    /// 操作结果。
    pub outcome: crate::FetchOutcome,

    /// 操作耗时，单位毫秒。
    pub latency_ms: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
