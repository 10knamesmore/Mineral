//! m20260906_stats 建表时的 searches 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "searches")]
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

    /// 搜索文本。
    #[sea_orm(column_type = "Text")]
    pub query: Option<String>,

    /// 搜索文本摘要。
    #[sea_orm(column_type = "Text")]
    pub query_hash: String,

    /// 搜索目标类型。
    #[sea_orm(column_type = "Text")]
    pub kind: String,

    /// 来源标识。
    #[sea_orm(column_type = "Text")]
    pub source: String,

    /// 请求页码。
    #[sea_orm(column_type = "Integer")]
    pub page: i64,

    /// 结果数量。
    #[sea_orm(column_type = "Integer")]
    pub result_count: Option<i64>,

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
