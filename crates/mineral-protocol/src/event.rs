//! server → client 主动推送的事件类型与属性树的协议面(observe 的 wire 形状)。

use std::sync::{Mutex, OnceLock};

use rustc_hash::FxHashSet;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::Subscription;

/// server → client 主动推送。状态值变更一律走 [`Event::PropertyChanged`],
/// 不为标量造 bespoke 事件;只有非属性化的生命周期事件(曲终 / 下载完成)留独立变体。
///
/// 下发按握手订阅集过滤(见 [`Event::subscription`]),client 不订阅的类别不会上 wire。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// 瞬时提示。`id` 相同则替换不堆叠(nvim `msg_show` 语义);`None` 为一次性堆叠。
    Toast {
        /// 视觉级别。
        kind: ToastKind,

        /// 单行人读文本。
        content: String,

        /// 顶替键:同 id 的存活提示被替换内容并续命;`None` 不参与顶替。
        id: Option<String>,

        /// 展示时长(秒);`None` 用 client 配置默认(`toast.flash_ttl_secs`)。
        ttl_secs: Option<u64>,
    },

    /// 属性树某项变更。「订阅即回放 + 末值合并」语义在 daemon 侧实现,此处只是线格式。
    PropertyChanged {
        /// 属性名(命名空间化,见 [`PropName`])。
        prop: PropName,

        /// 新值。
        value: PropValue,
    },

    /// 一首歌结束(生命周期事件,必带 reason)。
    TrackFinished {
        /// 结束的歌曲 id。
        song_id: mineral_model::SongId,

        /// 结束原因。
        reason: FinishReason,
    },

    /// 一首歌下载完成(永久导出落盘;已存在跳过**不**触发)。
    DownloadCompleted {
        /// 下载完成的歌曲 id。
        song_id: mineral_model::SongId,
    },

    /// per-song 持久 KV 某键变更(粗粒度:只报「哪首歌的哪个键」,值按需重读)。
    StoreChanged {
        /// 变更的歌曲 id。
        song_id: mineral_model::SongId,

        /// 变更的键(开放 key 或一等字段名,如 `rating`)。
        key: String,
    },
}

impl Event {
    /// 本事件所属的订阅类别 —— server 端据此按握手订阅集过滤下发。
    #[must_use]
    pub fn subscription(&self) -> Subscription {
        match self {
            Self::Toast { .. } => Subscription::Toast,
            Self::PropertyChanged { .. } => Subscription::Property,
            Self::TrackFinished { .. }
            | Self::DownloadCompleted { .. }
            | Self::StoreChanged { .. } => Subscription::Lifecycle,
        }
    }
}

/// Toast 视觉级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToastKind {
    /// 普通信息。
    Info,

    /// 警告。
    Warn,

    /// 错误。
    Error,
}

/// 曲目结束原因。Phase 1 只保证 `Eof` / `Skip` 可靠、`Stop` best-effort;
/// `Error` 随 player 级播放失败信号在 Phase 2 接入(变体先定形)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    /// 自然播完。
    Eof,

    /// 用户跳过(next / prev 切歌)。
    Skip,

    /// 解码 / 取链失败导致中断。
    Error,

    /// 用户显式停止。
    Stop,
}

/// 属性树键。仿 [`mineral_model::SourceKind`]:newtype + 关联常量,**开放**
/// (未知名经 [`PropName::from_name`] 运行时 intern)。身份只认内部字符串,故 `Copy`。
///
/// serde 表示是裸字符串名(序列化只写 name,反序列化按 name 解析,未知名 intern)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PropName(&'static str);

impl PropName {
    /// 当前在播歌(值为歌曲 id 的 `qualified()` 字符串;无在播为 `None`)。
    pub const PLAYER_SONG: Self = Self("player.song");

    /// 播放态(`"playing"` / `"paused"` / `"stopped"`)。
    pub const PLAYER_STATE: Self = Self("player.state");

    /// 音量百分比(0..=100)。
    pub const PLAYER_VOLUME: Self = Self("player.volume");

    /// 播放进度(整秒,daemon 侧整秒边界节流)。
    pub const PLAYER_POSITION: Self = Self("player.position");

    /// 播放模式(`PlayMode::script_name` 的蛇形名,如 `"sequential"`)。
    pub const PLAYER_MODE: Self = Self("player.mode");

    /// 队列长度。
    pub const QUEUE_LENGTH: Self = Self("queue.length");

    /// 按名字解析:命中内置常量则返回之,未知名 intern 成 `&'static str`(开放命名空间)。
    ///
    /// # Params:
    ///   - `name`: 属性名(与 [`as_str`](Self::as_str) 对称)
    ///
    /// # Return:
    ///   对应的 [`PropName`]。
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "player.song" => Self::PLAYER_SONG,
            "player.state" => Self::PLAYER_STATE,
            "player.volume" => Self::PLAYER_VOLUME,
            "player.position" => Self::PLAYER_POSITION,
            "player.mode" => Self::PLAYER_MODE,
            "queue.length" => Self::QUEUE_LENGTH,
            other => Self(intern(other)),
        }
    }

    /// 名字裸值(serde / 日志 / Lua 边界用)。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl Serialize for PropName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for PropName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name))
    }
}

/// 把一个运行时字符串固化成 `&'static str`,带去重池避免重复泄漏。
///
/// 仅在反序列化遇到未知属性名时走到;属性集合有界,泄漏有界。
/// (与 `mineral_model::source` 的 intern 同款实现 —— 各 crate 私有,不共享池。)
fn intern(s: &str) -> &'static str {
    static POOL: OnceLock<Mutex<FxHashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(FxHashSet::default()));
    // 中毒锁也能取回内部数据,不 panic。
    let mut guard = pool
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = guard.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// 属性值的协议化载荷。窄变体(Bool/Int/Str/None)覆盖本期全部可观测属性,
/// **不**引入 `serde_json::Value` 兜底;整首 `Song` 不上属性树(`player.song`
/// 只携带 qualified id 字符串,接收方按需另拉详情)。
///
/// 无浮点变体是有意的:f64 无 `Eq`(NaN 自反性陷阱),会堵死将来的去重 / 索引
/// 场景;首个真浮点属性出现时再追加变体(两端同仓同发版,零迁移成本)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropValue {
    /// 布尔。
    Bool(bool),

    /// 整数(volume / position 整秒 / queue.length 等)。
    Int(i64),

    /// 字符串(state / mode 名 / song 的 qualified id)。
    Str(String),

    /// 缺省 / 空(如无在播歌)。
    None,
}
