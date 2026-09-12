//! 查询启动所需的来源能力与脚本绑定。

use super::Client;
use crate::connection::Bootstrap;

impl Client {
    /// 拉一次启动自举数据(能力表 + 脚本绑定),CLI / TUI 连接后调用。
    pub async fn bootstrap(&self) -> Bootstrap {
        let channel_caps = self.channel_caps().await.into_success().unwrap_or_default();
        let script_binds = self.script_binds().await.into_success().unwrap_or_default();
        Bootstrap {
            channel_caps,
            script_binds,
        }
    }
}
