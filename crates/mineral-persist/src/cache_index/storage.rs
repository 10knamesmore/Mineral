//! 音频与封面缓存索引的实体读写。

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, Schema, Set,
};

use crate::entity::{audio_cache, cover_cache};

/// 缓存索引的用途，决定存储实体。
#[derive(Clone, Copy, Debug)]
pub(crate) enum CacheTable {
    /// 音频本体缓存。
    Audio,
    /// 封面缓存。
    Cover,
}

/// 从缓存索引读取的一行。
#[derive(FromQueryResult)]
pub(super) struct CacheRow {
    /// 缓存键。
    pub key: String,

    /// 相对缓存根目录的文件路径。
    pub relpath: String,

    /// 文件字节数。
    pub bytes: i64,

    /// 最近访问的逻辑时钟。
    pub last_access: i64,
}

impl CacheTable {
    /// 为日志提供稳定的缓存用途名称。
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Audio => "audio_cache",
            Self::Cover => "cover_cache",
        }
    }

    /// 建立索引表并载入已有记录。
    pub(super) async fn load(self, db: &DatabaseConnection) -> color_eyre::Result<Vec<CacheRow>> {
        let schema = Schema::new(DbBackend::Sqlite);
        let mut table = match self {
            Self::Audio => schema.create_table_from_entity(audio_cache::Entity),
            Self::Cover => schema.create_table_from_entity(cover_cache::Entity),
        };
        db.execute(table.if_not_exists()).await?;
        Ok(match self {
            Self::Audio => {
                audio_cache::Entity::find()
                    .into_model::<CacheRow>()
                    .all(db)
                    .await?
            }
            Self::Cover => {
                cover_cache::Entity::find()
                    .into_model::<CacheRow>()
                    .all(db)
                    .await?
            }
        })
    }

    /// 覆盖一条缓存记录，保留具名字段映射。
    pub(super) async fn upsert(
        self,
        db: &DatabaseConnection,
        row: CacheRow,
    ) -> color_eyre::Result<()> {
        match self {
            Self::Audio => {
                audio_cache::Entity::insert(audio_cache::ActiveModel {
                    key: Set(row.key),
                    relpath: Set(row.relpath),
                    bytes: Set(row.bytes),
                    last_access: Set(row.last_access),
                })
                .on_conflict(
                    OnConflict::column(audio_cache::Column::Key)
                        .update_columns([
                            audio_cache::Column::Relpath,
                            audio_cache::Column::Bytes,
                            audio_cache::Column::LastAccess,
                        ])
                        .to_owned(),
                )
                .exec_without_returning(db)
                .await?;
            }
            Self::Cover => {
                cover_cache::Entity::insert(cover_cache::ActiveModel {
                    key: Set(row.key),
                    relpath: Set(row.relpath),
                    bytes: Set(row.bytes),
                    last_access: Set(row.last_access),
                })
                .on_conflict(
                    OnConflict::column(cover_cache::Column::Key)
                        .update_columns([
                            cover_cache::Column::Relpath,
                            cover_cache::Column::Bytes,
                            cover_cache::Column::LastAccess,
                        ])
                        .to_owned(),
                )
                .exec_without_returning(db)
                .await?;
            }
        }
        Ok(())
    }

    /// 删除指定缓存键。
    pub(super) async fn delete(self, db: &DatabaseConnection, key: &str) -> color_eyre::Result<()> {
        match self {
            Self::Audio => {
                audio_cache::Entity::delete_by_id(key).exec(db).await?;
            }
            Self::Cover => {
                cover_cache::Entity::delete_by_id(key).exec(db).await?;
            }
        }
        Ok(())
    }

    /// 清空此用途的全部索引记录。
    pub(super) async fn clear(self, db: &DatabaseConnection) -> color_eyre::Result<()> {
        match self {
            Self::Audio => {
                audio_cache::Entity::delete_many().exec(db).await?;
            }
            Self::Cover => {
                cover_cache::Entity::delete_many().exec(db).await?;
            }
        }
        Ok(())
    }
}
