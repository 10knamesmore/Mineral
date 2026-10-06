//! daemon 脚本的 hook 注册、异步查询回执与命令出口。
//!
//! API 安装、文件求值与 daemon.lua 的 setup 发生在 VM 移交 [`crate::ScriptRuntime`] 之前。
//! Lua 闭包捕获宿主克隆，回调执行时从注册表取出函数后释放锁。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::Lua;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc::{UnboundedSender, error::SendError};

use crate::ScriptCmd;
use crate::api;
use crate::hooks::HookKind;

/// 按拦截点保存 hook，列表顺序就是调用顺序。
type HookRegistry = FxHashMap<HookKind, Vec<Arc<mlua::RegistryKey>>>;

/// 进程内不复用查询 ID，避免重载前的迟到结果命中新 VM 的回调。
static NEXT_QUERY_ID: AtomicU64 = AtomicU64::new(0);

/// 音乐查询的在途回调；结果到达时取出即删，只执行一次。
#[derive(Debug, Default)]
pub(crate) struct PendingQueries {
    /// 在途查询:id → Lua 回调。
    map: FxHashMap<u64, Arc<mlua::RegistryKey>>,
}

impl PendingQueries {
    /// 挂入一个回调,返回回投句柄。
    pub(crate) fn insert(&mut self, callback: Arc<mlua::RegistryKey>) -> crate::QueryId {
        let id = NEXT_QUERY_ID.fetch_add(1, Ordering::Relaxed);
        self.map.insert(id, callback);
        crate::QueryId(id)
    }

    /// 取出并移除一个回调；重复或未知查询返回 `None`。
    pub(crate) fn take(&mut self, query: crate::QueryId) -> Option<Arc<mlua::RegistryKey>> {
        self.map.remove(&query.0)
    }
}

/// daemon 命令出口；文件求值与 setup 期间暂存，VM 挂接成功后才提交。
#[derive(Clone, Debug)]
pub(crate) struct ScriptCommands {
    /// 已就绪 daemon 的命令队列。
    sender: UnboundedSender<ScriptCmd>,

    /// 待提交的入口命令；None 表示已经挂接，可直接发送。
    pending: Arc<Mutex<Option<Vec<ScriptCmd>>>>,
}

impl ScriptCommands {
    /// 入队一个命令；配置或 setup 失败被丢弃的宿主不会提交暂存命令。
    pub(crate) fn send(&self, command: ScriptCmd) -> Result<(), SendError<ScriptCmd>> {
        let mut pending = self.pending.lock();
        if let Some(commands) = pending.as_mut() {
            commands.push(command);
            Ok(())
        } else {
            self.sender.send(command)
        }
    }

    /// 回执入口就绪后按求值顺序提交命令；持锁避免运行期命令插队。
    pub(crate) fn activate(&self) {
        let mut pending = self.pending.lock();
        if let Some(commands) = pending.take() {
            for command in commands {
                let _ = self.sender.send(command);
            }
        }
    }
}

/// daemon Lua API 与回调调度共享的注册表和出方向通道。
#[derive(Clone, Debug)]
pub struct ScriptHost {
    /// 音乐拦截 hook，按注册顺序执行。
    pub(crate) hooks: Arc<Mutex<HookRegistry>>,

    /// 音乐查询回调的回执中转表。
    pub(crate) pending: Arc<Mutex<PendingQueries>>,

    /// 脚本 → daemon 的音乐命令与 session 配置覆盖。
    pub(crate) commands: ScriptCommands,

    /// daemon 回调失败类别的推送出口，不发送脚本界面命令。
    pub(crate) push: UnboundedSender<mineral_protocol::Event>,
}

impl ScriptHost {
    /// 构造 daemon 脚本宿主；命令暂存至 ScriptRuntime 挂接，配置或 setup 失败不提交。
    ///
    /// # Params:
    ///   - `commands`: daemon 独立任务消费的脚本命令出口
    ///   - `push`: daemon 事件中心接收的回调失败通知出口
    #[must_use]
    pub fn new(
        commands: UnboundedSender<ScriptCmd>,
        push: UnboundedSender<mineral_protocol::Event>,
    ) -> Self {
        Self {
            hooks: Arc::new(Mutex::new(HookRegistry::default())),
            pending: Arc::new(Mutex::new(PendingQueries::default())),
            commands: ScriptCommands {
                sender: commands,
                pending: Arc::new(Mutex::new(Some(Vec::new()))),
            },
            push,
        }
    }

    /// 保存音乐查询回调，返回随命令发送的回执句柄。
    pub(crate) fn register_query(
        &self,
        lua: &Lua,
        callback: mlua::Function,
    ) -> mlua::Result<crate::QueryId> {
        let key = Arc::new(lua.create_registry_value(callback)?);
        Ok(self.pending.lock().insert(key))
    }
}

/// 安装 `require("mineral.daemon")`；不创建全局 `mineral` 或 tui 模块。
///
/// # Params:
///   - `lua`: daemon.lua 配置与音乐回调的 VM，须在用户求值前安装
///   - `host`: 音乐命令出口与回调注册表
///
/// # Return:
///   安装成功；VM 表或闭包创建失败时返回 Lua 错误。
pub fn install_daemon_api(lua: &Lua, host: &ScriptHost) -> mlua::Result<()> {
    let mineral = lua.create_table()?;
    api::hook::install(lua, &mineral, host)?;
    api::download::install(lua, &mineral, host)?;
    api::player::install(lua, &mineral, host)?;
    api::log::install(lua, &mineral)?;
    api::sys::install(lua, &mineral)?;
    api::store::install(lua, &mineral, host)?;
    api::queue::install(lua, &mineral, host)?;
    api::library::install(lua, &mineral, host)?;
    let commands = host.commands.clone();
    api::config::install(lua, &mineral, move |ops| {
        let _ = commands.send(ScriptCmd::ConfigOverride { ops });
    })?;
    api::install_module(lua, "mineral.daemon", mineral)
}

/// 各源网页链接模板在 VM named registry 的键。
pub(crate) const WEB_URL_TEMPLATES: &str = "mineral.web_url_templates";

/// 一个 source 的网页链接模板，供模型投影生成 Lua `url` 字段。
///
/// `None` 表示该实体没有网页链接。占位语义见
/// [`mineral_channel_core::render_web_url`]。
#[derive(Clone, Debug)]
pub struct SourceWebUrls {
    /// 来源名(`SourceKind::name`)。
    pub source: String,

    /// 歌曲网页模板。
    pub song: Option<String>,

    /// 歌单网页模板。
    pub playlist: Option<String>,

    /// 专辑网页模板。
    pub album: Option<String>,

    /// 艺人网页模板。
    pub artist: Option<String>,
}

/// 为音乐实体投影写入网页链接模板；未写入的实体 `url` 为 nil。
///
/// # Params:
///   - `lua`: 当前宿主的 VM；daemon 在线程启动前写入，TUI 同步调用
///   - `entries`: 各 source 的模板集
pub fn seed_web_url_templates(
    lua: &Lua,
    entries: impl IntoIterator<Item = SourceWebUrls>,
) -> mlua::Result<()> {
    let table = lua.create_table()?;
    for urls in entries {
        let entry = lua.create_table()?;
        entry.set("song", urls.song)?;
        entry.set("playlist", urls.playlist)?;
        entry.set("album", urls.album)?;
        entry.set("artist", urls.artist)?;
        table.set(urls.source, entry)?;
    }
    lua.set_named_registry_value(WEB_URL_TEMPLATES, table)
}
