//! 脚本线程中的消息调度与 Lua 回调执行。
//!
//! 回调受看门狗保护；单个回调失败不影响同事件的其余回调，
//! 完整错误链写入日志并推送可顶替的错误提示。

mod callbacks;
mod projection;
mod return_value;
mod scheduler;
mod transforms;

pub(crate) use callbacks::report_callback_failure;
pub(crate) use projection::song_table;
pub(crate) use return_value::lua_field;
pub(crate) use scheduler::run_loop;
