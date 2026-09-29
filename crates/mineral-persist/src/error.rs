//! Failures at the persistent storage boundary.

use std::path::PathBuf;

/// A failed persistence operation.
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

    /// A database operation on a song's open KV key failed.
    #[error("{operation} for song {song} key {key} failed")]
    Key {
        /// Attempted KV operation.
        operation: &'static str,

        /// Song's stored identifier.
        song: String,

        /// KV key.
        key: String,

        /// Database failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// A cache index table operation failed.
    #[error("{operation} on {table} key {key:?} failed")]
    Cache {
        /// Cache index operation.
        operation: &'static str,

        /// Cache table name.
        table: &'static str,

        /// Affected key, if operating on a single entry.
        key: Option<String>,

        /// Database failure.
        #[source]
        source: sea_orm::DbErr,
    },

    /// Connecting to a SQLite file failed.
    #[error("cannot connect to SQLite at {}", path.display())]
    Connect {
        /// SQLite file path.
        path: PathBuf,

        /// Connection failure.
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

    /// Accessing a cache file failed.
    #[error("{operation} {} failed", path.display())]
    File {
        /// File operation.
        operation: &'static str,

        /// Target path.
        path: PathBuf,

        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },

    /// Copying a source file into the cache failed.
    #[error("cannot copy {} to {}", from.display(), to.display())]
    Copy {
        /// Source path.
        from: PathBuf,

        /// Destination path.
        to: PathBuf,

        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },

    /// The cache file task could not complete.
    #[error("{operation} task at {} failed", root.display())]
    Task {
        /// Cache operation.
        operation: &'static str,

        /// Cache root involved in the blocking task.
        root: PathBuf,

        /// Blocking task failure.
        #[source]
        source: tokio::task::JoinError,
    },

    /// A song's duration does not fit SQLite storage.
    #[error("duration for {song} cannot fit SQLite")]
    Duration {
        /// Qualified song identifier.
        song: String,

        /// Failed conversion.
        #[source]
        source: std::num::TryFromIntError,
    },

    /// A numeric value cannot fit into a database column or model field.
    #[error("numeric value outside supported range")]
    Number {
        /// Failed conversion.
        #[from]
        source: std::num::TryFromIntError,
    },

    /// A key reserved for a first-class field was used in the open KV store.
    #[error("reserved KV key: {key}")]
    ReservedKey {
        /// Rejected key.
        key: String,
    },

    /// A stored KV value does not match its type tag.
    #[error("invalid KV columns for song {song} key {key} type {value_type}")]
    InvalidValue {
        /// Song's stored identifier.
        song: String,

        /// KV key.
        key: String,

        /// Stored value type tag.
        value_type: String,
    },

    /// An increment targeted an existing non-integer value.
    #[error("KV key {key} does not hold an integer")]
    NotInteger {
        /// KV key.
        key: String,
    },

    /// A successful integer upsert did not return its value.
    #[error("KV integer value missing for key {key}")]
    MissingInteger {
        /// KV key.
        key: String,
    },

    /// A rating exceeds the supported maximum.
    #[error("rating {rating} exceeds maximum {max}")]
    InvalidRating {
        /// Supplied rating.
        rating: u8,

        /// Maximum allowed rating.
        max: u8,
    },
}
