//! Resolves song identities, acquires Apple playback assets and exposes clear AAC
//! packets alongside a shared, lazily assembled complete MP4 file.

pub(crate) use resolve::prepare;

mod assets;
mod cdn;
mod cenc;
mod error;
mod hls;
mod license;
mod mp4;
mod open;
mod resolve;
mod settings;
mod stream;
mod widevine;
