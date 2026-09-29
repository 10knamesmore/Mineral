//! Failures at this storage boundary.

/// A failed storage operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A SQLite operation failed.
    #[error("{operation} failed")]
    Database {
        /// Attempted operation and affected table.
        operation: &'static str,

        /// Database failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// A database operation on a named record failed.
    #[error("{operation} failed for {record}")]
    Record {
        /// Attempted operation and affected table.
        operation: &'static str,

        /// Affected song, playlist, source or cache key.
        record: String,

        /// Database failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// Applying a database schema failed.
    #[error("{database} schema migration failed; run `mineral cache reset --yes` to rebuild")]
    Migration {
        /// Database being migrated.
        database: &'static str,

        /// Migration failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// A numeric value cannot fit into a database column or model field.
    #[error("numeric value outside supported range")]
    Number {
        /// Failed conversion.
        #[from]
        source: std::num::TryFromIntError,
    },

    /// SQLite connection or file-cache operation failed.
    #[error(transparent)]
    Storage(#[from] mineral_persist::Error),
}

/// TUI storage operation result.
pub type Result<T> = std::result::Result<T, Error>;
