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

    /// Applying a database schema failed.
    #[error("{database} schema migration failed; run `mineral cache reset --yes` to rebuild")]
    Migration {
        /// Database being migrated.
        database: &'static str,

        /// Migration failure.
        #[source]
        source: sea_orm::DbErr,
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

    /// SQLite connection or file-cache operation failed.
    #[error(transparent)]
    Storage(#[from] mineral_persist::Error),

    /// An identifier belongs to another source.
    #[error("expected source {expected:?}, got {actual:?}")]
    NamespaceMismatch {
        /// Source bound to this store.
        expected: mineral_model::SourceKind,

        /// Source carried by the identifier.
        actual: mineral_model::SourceKind,
    },
}

/// Daemon storage operation result.
pub type Result<T> = std::result::Result<T, Error>;
