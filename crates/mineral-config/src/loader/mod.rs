//! 配置加载器:eval default / eval user / 深合并 / 反序列化 / 降级。

mod merge;
mod pipeline;
mod stub;
mod warning;

pub use pipeline::load;
pub use stub::inject_noop_host;
pub use warning::ConfigWarning;
