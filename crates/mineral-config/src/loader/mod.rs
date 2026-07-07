//! 配置加载器:eval default / eval user / 深合并 / 反序列化 / 降级。

mod lua_util;
mod merge;
mod pipeline;
mod stub;
mod tree;
mod warning;

pub use pipeline::{DaemonLoad, default_tree, load, load_with_vm};
pub use stub::inject_noop_host;
pub use tree::{from_tree, merge_tree, nest_path};
pub use warning::ConfigWarning;
