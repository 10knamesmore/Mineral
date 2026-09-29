//! Reads and writes a cache table initialized by its database owner.

use sea_orm::sea_query::{self, Alias, Expr, ExprTrait, Iden, OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, FromQueryResult};

/// Names a cache table without assigning an application-specific purpose.
#[derive(Clone, Copy, Debug)]
pub(super) struct CacheTable(pub(super) &'static str);

/// Columns shared by file-cache indexes.
#[derive(Iden)]
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

/// A persisted file-cache entry.
#[derive(FromQueryResult)]
pub(super) struct CacheRow {
    /// Cache identity.
    pub key: String,

    /// Path relative to the cache root.
    pub relpath: String,

    /// File size in bytes.
    pub bytes: i64,

    /// Logical access clock.
    pub last_access: i64,
}

impl CacheTable {
    /// Returns the table name for queries and diagnostics.
    pub(super) fn name(self) -> &'static str {
        self.0
    }

    /// Loads existing rows; table creation belongs to the database owner.
    pub(super) async fn load(self, db: &DatabaseConnection) -> crate::Result<Vec<CacheRow>> {
        let query = Query::select()
            .columns([
                CacheColumn::Key,
                CacheColumn::Relpath,
                CacheColumn::Bytes,
                CacheColumn::LastAccess,
            ])
            .from(Alias::new(self.0))
            .to_owned();
        CacheRow::find_by_statement(db.get_database_backend().build(&query))
            .all(db)
            .await
            .map_err(|source| crate::Error::Cache {
                operation: "load rows",
                table: self.0,
                key: None,
                source,
            })
    }

    /// Replaces one entry without changing the table schema.
    pub(super) async fn upsert(self, db: &DatabaseConnection, row: CacheRow) -> crate::Result<()> {
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
                row.bytes.into(),
                row.last_access.into(),
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
        db.execute(&query)
            .await
            .map_err(|source| crate::Error::Cache {
                operation: "upsert",
                table: self.0,
                key: Some(row.key),
                source,
            })?;
        Ok(())
    }

    /// Deletes one cache identity.
    pub(super) async fn delete(self, db: &DatabaseConnection, key: &str) -> crate::Result<()> {
        db.execute(
            Query::delete()
                .from_table(Alias::new(self.0))
                .and_where(Expr::col(CacheColumn::Key).eq(key)),
        )
        .await
        .map_err(|source| crate::Error::Cache {
            operation: "delete",
            table: self.0,
            key: Some(key.to_owned()),
            source,
        })?;
        Ok(())
    }

    /// Deletes all entries in this cache table.
    pub(super) async fn clear(self, db: &DatabaseConnection) -> crate::Result<()> {
        db.execute(Query::delete().from_table(Alias::new(self.0)))
            .await
            .map_err(|source| crate::Error::Cache {
                operation: "clear",
                table: self.0,
                key: None,
                source,
            })?;
        Ok(())
    }
}
