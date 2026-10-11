//! 歌单与专辑内光标位置的持久化实体。

use sea_orm::{
    ActiveModelBehavior, DeriveActiveEnum, DeriveEntityModel, DerivePrimaryKey, DeriveRelation,
    EntityTrait, EnumIter, PrimaryKeyTrait,
};

/// 光标位置所属的集合类型，映射为数据库中的固定文本值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum CollectionKind {
    /// 歌单内的位置。
    #[sea_orm(string_value = "playlist")]
    Playlist,

    /// 专辑内的位置。
    #[sea_orm(string_value = "album")]
    Album,
}

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "collection_pos")]
pub struct Model {
    /// 项目类型，区分同一来源内同值的歌单和专辑。
    #[sea_orm(primary_key, auto_increment = false)]
    pub collection_kind: CollectionKind,

    /// 项目来源。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub collection_namespace: String,

    /// 来源内项目身份。
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "Text")]
    pub collection_value: String,

    /// 歌曲来源。
    #[sea_orm(column_type = "Text")]
    pub song_namespace: String,

    /// 来源内歌曲身份。
    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    /// 选中条目的原始位置。
    #[sea_orm(column_type = "Integer")]
    pub sel_index: i64,

    /// 选中项在视口内的行位置。
    #[sea_orm(column_type = "Integer")]
    pub screen_row: i64,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
