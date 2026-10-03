//! 自建歌单的有序歌曲身份关系。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条持久记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "user_playlist_entries")]
pub struct Model {
    /// 所属自建歌单。
    #[sea_orm(primary_key, auto_increment = false)]
    pub playlist_id: String,

    /// 保存时的 0-based 队列位置。
    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,

    /// 歌曲来源，与歌单自身的 Mineral 来源无关。
    #[sea_orm(column_type = "Text")]
    pub song_namespace: String,

    /// 来源内歌曲身份，元数据由共享歌曲表提供。
    #[sea_orm(column_type = "Text")]
    pub song_value: String,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
