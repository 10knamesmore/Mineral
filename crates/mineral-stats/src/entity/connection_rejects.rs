//! connection_rejects 的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "connection_rejects")]
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

    /// 拒绝原因。
    pub reason: crate::RejectReason,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
