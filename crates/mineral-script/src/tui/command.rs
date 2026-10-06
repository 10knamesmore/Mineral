//! TUI Lua API 产生的本地界面效果与配置覆盖，不经过 daemon 协议。

use mineral_protocol::{TextSpan, ToastKind};

use crate::message::ConfigOverrideOp;

/// TUI 脚本的本地效果，由调用方在执行与配置校验都成功后提交。
#[derive(Clone, Debug, PartialEq)]
pub enum TuiCommand {
    /// 显示单行通知；同 id 顶替已有通知。
    Toast {
        /// 通知级别。
        kind: ToastKind,

        /// 行内文本与样式。
        content: Vec<TextSpan>,

        /// 顶替键，缺席时独立显示。
        id: Option<String>,

        /// 展示秒数，缺席时使用 TUI 配置。
        ttl_secs: Option<u64>,
    },

    /// 显示多行通知卡片。
    Card {
        /// 通知级别。
        kind: ToastKind,

        /// 标题，空列表表示不显示标题。
        title: Vec<TextSpan>,

        /// 有序行，每行由文本 spans 组成。
        body: Vec<Vec<TextSpan>>,

        /// 顶替键，缺席时独立显示。
        id: Option<String>,

        /// 展示秒数，缺席时驻留至用户关闭。
        ttl_secs: Option<u64>,
    },

    /// 覆盖本地窗口标题。
    WindowTitle {
        /// `None` 撤销覆盖，回落配置模板。
        text: Option<String>,
    },

    /// 覆盖当前 TUI 的本地配置，不读取或修改 daemon 配置。
    ConfigOverride {
        /// 一次调用的叶子操作，nil 撤销本地覆盖。
        ops: Vec<ConfigOverrideOp>,
    },
}
