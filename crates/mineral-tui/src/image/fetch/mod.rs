//! Client 端的封面源数据预取与按需解码 worker。

#[cfg(test)]
mod test_util;

mod decode;
mod pool;
mod preview;
mod source;
mod types;
mod worker;

pub(crate) use pool::CoverFetcher;
pub(crate) use types::{CoverCompletion, CoverPreviewReady, CoverReady, CoverRequestKind};
