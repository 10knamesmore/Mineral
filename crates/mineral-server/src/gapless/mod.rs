//! 服务端 gapless 编排：预排下一曲，并在无缝边界把它扶正为当前曲。

mod advance;
mod boundary;
mod prefetch;
mod state;

pub(crate) use advance::{Advance, adopt_queued, decide_advance};
pub(crate) use boundary::check_advance;
pub(crate) use prefetch::{arm_opened, check_prefetch, prefetch_source, record_prefetch};
pub(crate) use state::{PrefetchState, Queued};
