//! 可供 client 展示的后台失败类别；底层错误链只保留在 daemon 日志中。

use serde::{Deserialize, Serialize};

/// 后台异步操作失败后发送给订阅者的结构化提示，不包含内部诊断文本。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum FailureNotice {
    /// 会话配置覆盖的指定字段未通过校验，该字段未生效。
    #[error("config override rejected at {path}")]
    ConfigOverrideRejected {
        /// 未通过校验的配置字段路径。
        path: String,
    },

    /// 脚本重载失败，当前脚本是否仍可用由 `previous_kept` 标识。
    #[error("script reload failed (previous kept: {previous_kept})")]
    ScriptReloadFailed {
        /// 旧脚本是否仍在运行；否则脚本不可用。
        previous_kept: bool,
    },

    /// 配置文件重载失败；列出可定位的配置字段路径。
    #[error("config reload rejected at {fields:?}")]
    ConfigRejected {
        /// 未通过校验的字段路径；无法定位字段时为空。
        fields: Vec<String>,
    },

    /// 一次脚本回调执行失败，不影响同一事件的其他回调。
    #[error("script callback {callback} failed")]
    ScriptCallbackFailed {
        /// 回调所属事件或钩子的名称。
        callback: String,
    },

    /// 队列变换脚本运行失败，原队列未改变。
    #[error("queue transform failed")]
    QueueTransformFailed,

    /// 队列变换返回了无法映射到当前队列的歌曲。
    #[error("queue transform returned an invalid song")]
    QueueTransformInvalidSong,

    /// 指定歌单的下载准备失败。
    #[error("playlist download failed for {id:?}")]
    PlaylistDownloadFailed {
        /// 下载失败的歌单身份。
        id: mineral_model::PlaylistId,
    },
}
