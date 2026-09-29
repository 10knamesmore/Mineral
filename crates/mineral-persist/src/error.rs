//! Failures at this storage boundary.

use std::path::PathBuf;

/// A failed storage operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A cache statement could not be constructed.
    #[error(transparent)]
    Statement(#[from] sea_orm::sea_query::error::Error),

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

    /// A numeric value cannot fit into a database column or model field.
    #[error("numeric value outside supported range")]
    Number {
        /// Failed conversion.
        #[from]
        source: std::num::TryFromIntError,
    },
}
