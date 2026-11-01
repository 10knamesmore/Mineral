//! daemon 会话处理：连接登记、握手、有序请求执行与按订阅主题的状态发布。

mod artist_albums;
mod connections;
pub(crate) mod dispatch;
mod playlist;
mod search;
mod session;
mod subscriptions;
mod updates;

pub(crate) use connections::ConnectionCounters;
pub(crate) use session::{SessionServices, run};
