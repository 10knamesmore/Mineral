use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 专辑列表中一个位置的完整元信息，不保存曲目。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "my_albums_cache_items")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub namespace: String,

    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,

    #[sea_orm(column_type = "Text")]
    pub album_id: String,

    #[sea_orm(column_type = "Text")]
    pub name: String,

    #[sea_orm(column_type = "Text")]
    pub description: String,

    #[sea_orm(column_type = "Text")]
    pub company: Option<String>,

    pub publish_time_ms: Option<i64>,

    pub track_count: Option<i64>,

    #[sea_orm(column_type = "Text")]
    pub cover_url: Option<String>,

    #[sea_orm(column_type = "Text")]
    pub web_url: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::requested_albums::Entity",
        from = "Column::Namespace",
        to = "super::requested_albums::Column::Namespace",
        on_delete = "Cascade"
    )]
    Snapshot,
}

impl ActiveModelBehavior for ActiveModel {}
