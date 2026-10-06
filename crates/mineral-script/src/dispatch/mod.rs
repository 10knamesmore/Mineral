//! 脚本线程中的消息调度与 Lua 回调执行。
//!
//! 音乐查询、hook 和配置函数受看门狗保护；失败记录完整错误链，
//! 需要通知 client 时只推送结构化失败类别。

mod callbacks;
mod return_value;
mod scheduler;
mod transforms;

pub(crate) use callbacks::report_callback_failure;
pub(crate) use return_value::lua_field;
pub(crate) use scheduler::run_loop;
