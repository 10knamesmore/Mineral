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
    /// 瞬时提示(单行 flash,TTL 自动退场)。`id` 相同则替换不堆叠
    /// (nvim `msg_show` 语义);`None` 为一次性堆叠。
    Toast {
        /// 视觉级别。
        kind: ToastKind,

        /// 人读内容(行内 spans,纯文本即单个全默认 span)。单行语义:
        /// span 文本内嵌换行时 client 截首行。
        content: Vec<TextSpan>,

        /// 顶替键:同 id 的存活提示被替换内容并续命;`None` 不参与顶替。
        id: Option<String>,

        /// 展示时长(秒);`None` 用 client 配置默认(`toast.flash_ttl_secs`)。
        ttl_secs: Option<u64>,
    },

    /// 多行通知卡片(标题与 body 都带行内样式)。`id` 相同则替换不堆叠
    /// (与 [`Event::Toast`] 同款顶替语义)。
    Card {
        /// 视觉级别(client 据此选边框 / 标题色)。
        kind: ToastKind,

        /// 顶替键:同 id 的存活卡片被替换内容(退场中复活);`None` 不参与顶替。
        id: Option<String>,

        /// 标题(client 画进卡片边框,行内 spans);空 = 不画。
        title: Vec<TextSpan>,

        /// 卡片正文:外层 = 行,内层 = 行内 spans。
        body: Vec<Vec<TextSpan>>,

        /// 展示时长(秒):`Some` 到时自动退场(与 toast 同款);
        /// `None` 驻留,用户显式关闭才退场。
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

    /// 脚本已热重载(config.lua 变更、新 VM 顶上)。client 据此重拉
    /// `ScriptBinds` 合 keymap(daemon 重载完成是 bind 表就绪的权威信号)。
    ScriptReloaded,

    /// 自定义事件总线消息(脚本 `mineral.emit` 扇出)。daemon 零解释、
    /// 原样转发;语义契约在用户自己的两端(脚本 ↔ 外部 client)之间。
    BusMessage {
        /// 事件名(用户命名空间,建议 `插件名.事件` 形)。
        name: String,

        /// 开放载荷(树形自描述,见 [`BusValue`])。
        payload: BusValue,
    },

    /// 脚本对某 UI 旋钮的 session 级覆盖(`mineral.ui.override`)。daemon
    /// 零解释:只存表 + 转发 + 新 client 握手重放;key→旋钮的类型化映射在
    /// client 边缘做,未知 key client 侧 warn + 丢。
    UiOverride {
        /// 旋钮键(约定 = 配置路径,如 `lyrics.fullscreen_line_gap`)。
        key: String,

        /// 覆盖值;`None` = 撤销覆盖,client 回落自己的配置值。
        value: Option<BusValue>,
    },
}

impl Event {
    /// 本事件所属的订阅类别 —— server 端据此按握手订阅集过滤下发。
    #[must_use]
    pub fn subscription(&self) -> Subscription {
        match self {
            Self::Toast { .. } | Self::Card { .. } => Subscription::Toast,
            Self::PropertyChanged { .. } => Subscription::Property,
            Self::TrackFinished { .. }
            | Self::DownloadCompleted { .. }
            | Self::StoreChanged { .. }
            | Self::ScriptReloaded => Subscription::Lifecycle,
            Self::BusMessage { .. } => Subscription::Bus,
            Self::UiOverride { .. } => Subscription::UiOverride,
        }
    }
}

/// 自定义事件总线的开放载荷:用户自定义结构,daemon 零解释转发。
///
/// 树形自描述值,bincode 可编解码(不依赖 `deserialize_any`,故**不用**
/// `serde_json::Value`);Lua table ↔ 本类型的转换在脚本 crate 的 VM 边界。
/// `Map` 用有序键值对(非 hash 容器):编码确定、保留脚本侧构造顺序。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BusValue {
    /// 空。
    Nil,

    /// 布尔。
    Bool(bool),

    /// 整数。
    Int(i64),

    /// 浮点(Lua number 的非整数形)。
    Float(f64),

    /// 字符串。
    Str(String),

    /// 数组。
    Array(Vec<Self>),

    /// 字符串键映射(有序键值对)。
    Map(Vec<(String, Self)>),
}

/// 一段行内文本 + 样式(样式全缺省 = 正文默认、靠左),通知类载荷
/// (toast 内容 / 卡片标题 / 卡片 body)通用的文本单元。
///
/// 样式是协议面的**语义**描述:fg 用主题角色(client 按当前主题落色),
/// 修饰位逐项布尔 —— 不在 wire 上传终端转义序列。`align` 把同一行的 spans
/// 分成左 / 中 / 右三段(`|左段    中段    右段|`),段内按原顺序连排
/// (单行 flash / 卡片标题这类非整行语境忽略 `align`)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSpan {
    /// 文本内容。
    pub text: String,

    /// 前景色;`None` 用正文默认色。
    pub fg: Option<SpanFg>,

    /// 加粗。
    pub bold: bool,

    /// 斜体。
    pub italic: bool,

    /// 下划线。
    pub underline: bool,

    /// 弱化(降亮度)。
    pub dim: bool,

    /// 行内水平段位(缺省靠左)。
    pub align: SpanAlign,
}

impl TextSpan {
    /// 纯文本 span(无任何样式、靠左)。
    ///
    /// # Params:
    ///   - `text`: 文本内容
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: None,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            align: SpanAlign::Left,
        }
    }
}

/// span 的行内水平段位:同一行按段位分三组各自对齐。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanAlign {
    /// 贴行左缘(缺省)。
    #[default]
    Left,

    /// 行内居中。
    Center,

    /// 贴行右缘。
    Right,
}

/// span 前景色:主题角色(client 按当前主题落色,换主题不破相)或直给 RGB。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanFg {
    /// 正文色。
    Text,

    /// 次级文本。
    Subtext,

    /// 弱化 / 提示。
    Overlay,

    /// 强调色。
    Accent,

    /// 红(错误 / 危险)。
    Red,

    /// 黄(警告)。
    Yellow,

    /// 绿(成功)。
    Green,

    /// 橙(柔和强调)。
    Peach,

    /// 直给 RGB(不随主题)。
    Rgb(u8, u8, u8),
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

    /// 终端 UI 状态(复合属性:`rows` / `cols` / `fullscreen`;无 client 在线为 `None`)。
    pub const TERMINAL: Self = Self("terminal");

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
            "terminal" => Self::TERMINAL,
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

    /// 复合结构(有序键值对,如 `terminal` 的 rows/cols/fullscreen)。
    /// 递归用自身而非 [`BusValue`] 是有意的:BusValue 带 `Float` 无 `Eq`,
    /// 会破坏属性树「与上次值比较只发真变更」的 diff 语义。
    Table(Vec<(String, Self)>),

    /// 缺省 / 空(如无在播歌)。
    None,
}
