use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "my_albums_cache_artists")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub namespace: String,

    #[sea_orm(primary_key, auto_increment = false)]
    pub album_position: i64,

    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,

    #[sea_orm(column_type = "Text")]
    pub artist_id: String,

    #[sea_orm(column_type = "Text")]
    pub name: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::albums::Entity",
        from = "(Column::Namespace, Column::AlbumPosition)",
        to = "(super::albums::Column::Namespace, super::albums::Column::Position)",
        on_delete = "Cascade"
    )]
    Album,
}

impl ActiveModelBehavior for ActiveModel {}
