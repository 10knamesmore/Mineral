//! daemon ↔ 脚本线程之间的内部消息类型。
//!
//! 方向约定:[`ScriptEvent`] 是 daemon → 脚本(投递给 Lua 回调的事件),
//! [`ScriptCmd`] 是脚本 → daemon(Lua API 发出的播放器命令)。两侧都是
//! **结构化** Rust 类型,Lua 字符串只出现在 VM 边界的适配层(`api` 模块)。

use mineral_model::{Song, SongId};
use mineral_protocol::PlayMode;
use tokio::sync::mpsc::UnboundedSender;

/// daemon 投递给脚本线程的事件。携带 daemon 侧已有的完整模型
/// (如整个 [`Song`]),投影成 Lua table 的裁剪发生在 dispatch 层。
#[derive(Clone, Debug)]
pub enum ScriptEvent {
    /// 一首歌结束(必带 reason,与 wire 的 `FinishReason` 同构)。
    TrackFinished {
        /// 结束的歌曲。
        song: Box<Song>,

        /// 结束原因。
        reason: TrackFinishedReason,
    },

    /// 一首歌下载完成(永久导出落盘;已存在跳过不触发)。
    DownloadCompleted {
        /// 下载完成的歌曲。
        song: Box<Song>,

        /// 落盘路径。
        path: std::path::PathBuf,
    },

    /// 属性树某项变更(PR-3 接 `mineral.observe` 后真正消费;变体先定形)。
    PropertyChanged {
        /// 属性键。
        key: PropKey,

        /// 新值。
        value: PropValue,
    },
}

/// 曲目结束原因(内部表示,与 `mineral_protocol::FinishReason` 同构;
/// 不直接复用是为了脚本层不感知 wire 演进)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackFinishedReason {
    /// 自然播完。
    Eof,

    /// 用户跳过(next / prev 切歌)。
    Skip,

    /// 解码 / 取链失败导致中断。
    Error,

    /// 用户显式停止。
    Stop,
}

impl TrackFinishedReason {
    /// 给 Lua 回调的字符串表示。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eof => "eof",
            Self::Skip => "skip",
            Self::Error => "error",
            Self::Stop => "stop",
        }
    }
}

/// 可观测属性键(**封闭**枚举,与 `mineral_protocol::PropName` 的六个内置常量
/// 一一对应)。protocol 侧 `PropName` 为前向兼容保持开放;脚本侧 observe 必须
/// 校验合法名,故这里收成封闭集合。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropKey {
    /// 当前在播歌(qualified id 字符串;无在播为 none)。
    PlayerSong,

    /// 播放态("playing" / "paused" / "stopped")。
    PlayerState,

    /// 音量百分比(0..=100)。
    PlayerVolume,

    /// 播放进度(整秒)。
    PlayerPosition,

    /// 播放模式(`PlayMode` 稳定名)。
    PlayerMode,

    /// 队列长度。
    QueueLength,
}

impl PropKey {
    /// 全部属性键(`mineral.observe` 错误信息 / meta 守卫测试用)。
    pub const ALL: [Self; 6] = [
        Self::PlayerSong,
        Self::PlayerState,
        Self::PlayerVolume,
        Self::PlayerPosition,
        Self::PlayerMode,
        Self::QueueLength,
    ];

    /// 按属性名解析(与 [`Self::as_str`] 对偶);未知名为 `None`。
    ///
    /// # Params:
    ///   - `name`: 属性名字符串(脚本侧输入)
    ///
    /// # Return:
    ///   对应键;未知名为 `None`,调用方报脚本错误。
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "player.song" => Some(Self::PlayerSong),
            "player.state" => Some(Self::PlayerState),
            "player.volume" => Some(Self::PlayerVolume),
            "player.position" => Some(Self::PlayerPosition),
            "player.mode" => Some(Self::PlayerMode),
            "queue.length" => Some(Self::QueueLength),
            _ => None,
        }
    }

    /// 属性名字符串(与 `PropName` 的内置常量字面量一致)。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlayerSong => "player.song",
            Self::PlayerState => "player.state",
            Self::PlayerVolume => "player.volume",
            Self::PlayerPosition => "player.position",
            Self::PlayerMode => "player.mode",
            Self::QueueLength => "queue.length",
        }
    }
}

/// 属性值(内部表示)。与 `mineral_protocol::PropValue` 同构但独立定形,
/// 理由同 [`TrackFinishedReason`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropValue {
    /// 整数(volume / position 整秒 / queue.length)。
    Int(i64),

    /// 字符串(state / mode 名 / song 的 qualified id)。
    Str(String),

    /// 缺省 / 空(如无在播歌)。
    None,
}

/// 脚本发往 daemon 的播放器命令。daemon 侧由独立 task drain 并落到
/// player / download 执行面(PR-4 接线)。
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptCmd {
    /// 播放 / 暂停切换。
    Toggle,

    /// 下一首。
    Next,

    /// 上一首。
    Prev,

    /// 停止播放。
    Stop,

    /// 相对 seek(秒,可负)。
    SeekRel(f64),

    /// 绝对 seek(秒)。
    SeekTo(f64),

    /// 设音量(0..=100)。
    SetVolume(u8),

    /// 设播放模式。
    SetMode(PlayMode),

    /// 播放指定歌曲。
    Play(SongId),

    /// 下载指定歌曲。
    Download(SongId),
}

/// 脚本线程主循环消费的信封:事件投递、动作调用或停机。
#[derive(Debug)]
pub(crate) enum ScriptMsg {
    /// 投递一个事件给已注册的 Lua 回调。
    Event(ScriptEvent),

    /// 调用一个具名动作(`mineral.action` 注册),结果经 oneshot 回执。
    Action {
        /// 动作注册名。
        name: String,

        /// 调用结果回执(接收端 drop 时静默丢)。
        reply: tokio::sync::oneshot::Sender<ActionOutcome>,
    },

    /// 优雅停机:主循环退出,线程结束。
    Stop,
}

/// 一次具名动作调用的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// 回调执行完成。
    Done,

    /// 该名字未注册。
    NotFound,

    /// 回调执行失败(Lua 错误 / 超看门狗硬阈值被中断),携带单行错误信息。
    Failed(String),
}

/// daemon 侧持有的事件投递句柄(fire-and-forget)。
///
/// 发送失败(脚本线程已退出)静默丢弃 —— 脚本是旁路增强,不反压播放主链路。
#[derive(Clone, Debug)]
pub struct ScriptSender(pub(crate) UnboundedSender<ScriptMsg>);

impl ScriptSender {
    /// 投递一个事件给脚本线程。
    ///
    /// # Params:
    ///   - `event`: 要投递的事件
    pub fn send(&self, event: ScriptEvent) {
        // 接收端 Drop(脚本线程退出)时丢弃即可,不是错误。
        let _ = self.0.send(ScriptMsg::Event(event));
    }

    /// 调用一个具名动作,返回结果回执的接收端。
    ///
    /// 脚本线程已退出时,回执立即就绪为 [`ActionOutcome::Failed`]。
    ///
    /// # Params:
    ///   - `name`: 动作注册名
    ///
    /// # Return:
    ///   oneshot 接收端;`await` 得到调用结果。
    #[must_use]
    pub fn invoke_action(&self, name: String) -> tokio::sync::oneshot::Receiver<ActionOutcome> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        if let Err(send_failed) = self.0.send(ScriptMsg::Action { name, reply }) {
            // 线程已退出:取回 reply 端立即回执失败,调用方不会空等。
            if let ScriptMsg::Action { reply, .. } = send_failed.0 {
                let _ = reply.send(ActionOutcome::Failed("脚本线程已退出".to_owned()));
            }
        }
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::TrackFinishedReason;

    #[test]
    fn meta_stub_finish_reason_alias_matches_rust() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        // meta/mineral.lua 的 `mineral.FinishReason` 字符串枚举必须与
        // Rust 侧 `as_str` 的全部取值逐字一致(顺序也钉死)。
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        let literals = [
            TrackFinishedReason::Eof,
            TrackFinishedReason::Skip,
            TrackFinishedReason::Error,
            TrackFinishedReason::Stop,
        ]
        .map(|reason| format!("\"{}\"", reason.as_str()))
        .join("|");
        let alias = format!("---@alias mineral.FinishReason {literals}");
        assert!(
            meta.contains(&alias),
            "meta stub 缺少与 Rust 一致的别名行:`{alias}`"
        );
        Ok(())
    }
}
