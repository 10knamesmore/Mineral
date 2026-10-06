//! 宿主无关的配置求值、合并、落型和字段诊断。

mod lua_util;
mod merge;
mod pipeline;
mod tree;
mod warning;

pub use lua_util::{extract_setup, table_path};
pub use pipeline::{FileLoad, defaults, from_source, load_file};
pub use tree::{deserialize_tree, merge_tree, nest_path};
pub use warning::ConfigWarning;
