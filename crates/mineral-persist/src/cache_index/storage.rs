//! Reads and writes a cache table initialized by its database owner.

use mineral_common::revision::Revision;
use sea_orm::sea_query::{Alias, Expr, ExprTrait, OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};

use crate::{Error, Result};

/// Names a cache table without assigning an application-specific purpose.
#[derive(Clone, Copy, Debug)]
pub(super) struct CacheTable(pub(super) &'static str);

/// Columns shared by file-cache indexes.
#[derive(DeriveIden)]
enum CacheColumn {
    /// Cache identity.
    Key,

    /// Path relative to the cache root.
    Relpath,

    /// File size in bytes.
    Bytes,

    /// Logical access clock.
    LastAccess,
}

/// Cache entry exchanged with the index; SQL integer conversion stays in this module.
pub(super) struct CacheRow {
    /// Cache identity.
    pub key: String,

    /// Path relative to the cache root.
    pub relpath: String,

    /// File size in bytes.
    pub bytes: u64,

    /// Logical access clock.
    pub last_access: Revision,
}

impl CacheTable {
    /// Returns the table name for queries and diagnostics.
    pub(super) fn name(self) -> &'static str {
        self.0
    }

    /// Loads existing rows; table creation belongs to the database owner.
    pub(super) async fn load(self, db: &DatabaseConnection) -> Result<Vec<CacheRow>> {
        let query = Query::select()
            .columns([
                CacheColumn::Key,
                CacheColumn::Relpath,
                CacheColumn::Bytes,
                CacheColumn::LastAccess,
            ])
            .from(Alias::new(self.0))
            .to_owned();
        db.query_all(&query)
            .await
            .map_err(|source| Error::Cache {
                operation: "load rows",
                table: self.0,
                key: None,
                source,
            })?
            .into_iter()
            .map(|row| {
                let (key, relpath, bytes, last_access) = row
                    .try_get_many_by_index::<(String, String, i64, i64)>()
                    .map_err(|source| Error::Cache {
                        operation: "decode row",
                        table: self.0,
                        key: None,
                        source,
                    })?;
                Ok(CacheRow {
                    key,
                    relpath,
                    bytes: u64::try_from(bytes)?,
                    last_access: Revision::try_from(last_access)?,
                })
            })
            .collect()
    }

    /// Replaces one entry without changing the table schema.
    pub(super) async fn upsert(self, db: &DatabaseConnection, row: CacheRow) -> Result<()> {
        let query = Query::insert()
            .into_table(Alias::new(self.0))
            .columns([
                CacheColumn::Key,
                CacheColumn::Relpath,
                CacheColumn::Bytes,
                CacheColumn::LastAccess,
            ])
            .values([
                row.key.clone().into(),
                row.relpath.into(),
                i64::try_from(row.bytes)?.into(),
                i64::try_from(row.last_access)?.into(),
            ])?
            .on_conflict(
                OnConflict::column(CacheColumn::Key)
                    .update_columns([
                        CacheColumn::Relpath,
                        CacheColumn::Bytes,
                        CacheColumn::LastAccess,
                    ])
                    .to_owned(),
            )
            .to_owned();
        db.execute(&query).await.map_err(|source| Error::Cache {
            operation: "upsert",
            table: self.0,
            key: Some(row.key),
            source,
        })?;
        Ok(())
    }

    /// Deletes one cache identity.
    pub(super) async fn delete(self, db: &DatabaseConnection, key: &str) -> Result<()> {
        db.execute(
            Query::delete()
                .from_table(Alias::new(self.0))
                .and_where(Expr::col(CacheColumn::Key).eq(key)),
        )
        .await
        .map_err(|source| Error::Cache {
            operation: "delete",
            table: self.0,
            key: Some(key.to_owned()),
            source,
        })?;
        Ok(())
    }

    /// Deletes all entries in this cache table.
    pub(super) async fn clear(self, db: &DatabaseConnection) -> Result<()> {
        db.execute(Query::delete().from_table(Alias::new(self.0)))
            .await
            .map_err(|source| Error::Cache {
                operation: "clear",
                table: self.0,
                key: None,
                source,
            })?;
        Ok(())
    }
}
