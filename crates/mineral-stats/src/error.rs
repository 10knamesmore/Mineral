//! Failures at the statistics storage boundary.

use std::path::PathBuf;

/// A failed statistics storage operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A SQLite operation failed.
    #[error("{operation} failed")]
    Database {
        /// Attempted query, write, or transaction operation.
        operation: &'static str,

        /// SQLite failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// A SQLite operation on a named event table failed.
    #[error("{operation} on {table} failed")]
    EventTable {
        /// Attempted operation.
        operation: &'static str,

        /// Event table name.
        table: &'static str,

        /// SQLite failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// A SQLite operation on a song failed.
    #[error("{operation} for song {song} failed")]
    Song {
        /// Attempted operation.
        operation: &'static str,

        /// Song's stored identifier.
        song: String,

        /// SQLite failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// Opening a statistics database failed.
    #[error("cannot open statistics database at {}", path.display())]
    Open {
        /// Database file path.
        path: PathBuf,

        /// Connection failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// Applying the statistics schema failed.
    #[error("statistics schema migration failed")]
    Migration {
        /// Migration failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// An aggregate query unexpectedly returned no row.
    #[error("{query} aggregate did not return a row")]
    MissingAggregate {
        /// Aggregate query name.
        query: &'static str,
    },

    /// Counting events unexpectedly returned no row.
    #[error("event count did not return a row for {table}")]
    MissingEventCount {
        /// Event table name.
        table: &'static str,
    },

    /// A distribution query for one dimension failed.
    #[error("distribution query for {column} failed")]
    Distribution {
        /// Dimension column being queried.
        column: String,

        /// SQLite failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// A numeric result cannot fit in the report's column type.
    #[error("statistics numeric result outside supported range")]
    Number {
        /// Failed conversion.
        #[from]
        source: std::num::TryFromIntError,
    },
}
