use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 歌单列表中一个位置的完整元信息，不保存 membership。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "my_playlists_cache_items")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub namespace: String,

    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,

    #[sea_orm(column_type = "Text")]
    pub playlist_id: String,

    #[sea_orm(column_type = "Text")]
    pub name: String,

    pub can_rename: bool,

    pub can_delete: bool,

    #[sea_orm(column_type = "Text")]
    pub description: String,

    #[sea_orm(column_type = "Text")]
    pub cover_url: Option<String>,

    #[sea_orm(column_type = "Text")]
    pub web_url: Option<String>,

    pub track_count: i64,

    pub play_count: Option<i64>,

    pub subscriber_count: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::requested_playlists::Entity",
        from = "Column::Namespace",
        to = "super::requested_playlists::Column::Namespace",
        on_delete = "Cascade"
    )]
    Snapshot,
}

impl ActiveModelBehavior for ActiveModel {}
