//! `mineral.timer.*`:脚本线程内定时器。
//!
//! 不另起线程 / tokio:挂在脚本线程主循环的 `recv_timeout` 心跳上
//! (无定时器时长等消息,零空转)。回调与事件回调同走看门狗熔断。

pub(crate) mod after;
pub(crate) mod every;
mod lua_table;
pub(crate) mod table;

pub(crate) use lua_table::install;
