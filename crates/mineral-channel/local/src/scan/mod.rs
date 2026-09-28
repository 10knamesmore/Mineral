//! Rust filesystem scanning and direct-directory catalog building.

mod context;
mod execution;
mod identity;
mod projection;

pub(crate) use execution::run;
pub(crate) use projection::build;
