//! 搜索页面、按 source 保存的会话与按 kind 分桶的结果。

mod page;
mod results;
mod session;

pub use page::{PromptSegment, SearchFocus, SearchPage};
pub use results::KindResults;
pub use session::SearchSession;
