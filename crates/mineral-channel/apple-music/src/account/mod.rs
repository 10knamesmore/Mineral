//! Account validation and authenticated requests shared by metadata and playback.
//! Credentials are persisted separately from the lifetime of an HTTP client.

mod client;
mod credentials;
mod developer_token;
mod response;

pub(crate) use client::AccountClient;
pub use credentials::{StoredAppleMusicAuth, load_stored};
pub(crate) use credentials::{from_token, save_login};
pub(crate) use response::SESSION_ENDED_FAILURE;
