//! daemon 会话处理：连接登记、握手、有序请求执行与按订阅主题的状态发布。

mod dispatch;
mod registry;
mod session;
mod subscriptions;
mod updates;

pub(crate) use registry::ConnRegistry;
pub(crate) use session::{SessionServices, run};
