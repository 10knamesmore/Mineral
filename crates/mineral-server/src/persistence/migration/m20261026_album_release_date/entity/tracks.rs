use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "album_cache_tracks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub namespace: String,

    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub album_id: String,

    #[sea_orm(primary_key, auto_increment = false)]
    pub collection_index: i64,

    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    #[sea_orm(column_type = "Text")]
    pub name: String,

    #[sea_orm(column_type = "Text")]
    pub alias: Option<String>,

    #[sea_orm(column_type = "Text")]
    pub song_album_id: Option<String>,

    #[sea_orm(column_type = "Text")]
    pub song_album_name: Option<String>,

    pub duration_ms: Option<i64>,

    #[sea_orm(column_type = "Text")]
    pub cover_url: Option<String>,

    #[sea_orm(column_type = "Text")]
    pub source_url: Option<String>,

    pub unavailable: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::albums::Entity",
        from = "(Column::Namespace, Column::AlbumId)",
        to = "(super::albums::Column::Namespace, super::albums::Column::AlbumId)",
        on_delete = "Cascade"
    )]
    Album,
}

impl ActiveModelBehavior for ActiveModel {}
