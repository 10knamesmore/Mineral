//! 脚本宿主侧句柄:回调注册表 + 出方向通道,以及把 `mineral.*` API
//! 挂进 VM 的安装入口。
//!
//! 接线顺序(daemon 侧):`ScriptHost::new` → [`install_api`] → eval 用户脚本
//! → `ScriptRuntime::spawn` 把 VM 移交脚本线程。安装与求值都发生在移交前,
//! 故 API 闭包捕获的是 [`ScriptHost`] 的克隆,线程间共享同一注册表。

use std::sync::Arc;

use mlua::Lua;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc::UnboundedSender;

use crate::ScriptCmd;
use crate::api;
use crate::message::{PropKey, PropValue};

/// 已注册的 Lua 回调与属性缓存(按事件 / 属性分桶,注册顺序即调用顺序)。
///
/// 回调存 `Arc<RegistryKey>` 而非 `Function`:dispatch 时锁内克隆 Arc 列表、
/// **锁外**取函数并调用 —— 回调里再调 `mineral.on` / `mineral.observe`
/// 也不会撞 parking_lot 的不可重入死锁。
#[derive(Debug, Default)]
pub(crate) struct EventRegistry {
    /// `track_finished` 回调。
    pub(crate) track_finished: Vec<Arc<mlua::RegistryKey>>,

    /// `download_completed` 回调。
    pub(crate) download_completed: Vec<Arc<mlua::RegistryKey>>,

    /// 属性观察者(`mineral.observe`,按属性键分桶)。
    pub(crate) observers: FxHashMap<PropKey, Vec<Arc<mlua::RegistryKey>>>,

    /// 属性当前值缓存:daemon 每次投递 `PropertyChanged` 时更新;
    /// `observe` 注册时有值即回放、`mineral.get` 同源读。
    pub(crate) props: FxHashMap<PropKey, PropValue>,

    /// 具名动作(`mineral.action`,重名注册报 Lua 错)。
    /// 触发面(client → daemon → 这里)是 PR-4 的事,本期只注册。
    pub(crate) actions: FxHashMap<String, Arc<mlua::RegistryKey>>,
}

/// 脚本宿主句柄:Lua API 闭包与 dispatch 层共享的全部可变面。
#[derive(Clone, Debug)]
pub struct ScriptHost {
    /// 事件回调注册表。
    pub(crate) events: Arc<Mutex<EventRegistry>>,

    /// 脚本 → daemon 的命令出口(`mineral.player.*` / `mineral.download`)。
    pub(crate) commands: UnboundedSender<ScriptCmd>,

    /// 脚本 → client 的推送出口(toast 经 daemon event hub 下发)。
    pub(crate) push: UnboundedSender<mineral_protocol::Event>,
}

impl ScriptHost {
    /// 构造宿主句柄。
    ///
    /// # Params:
    ///   - `commands`: 脚本命令出口(daemon 侧独立 task drain)
    ///   - `push`: 推送出口(daemon 侧汇入 event hub)
    #[must_use]
    pub fn new(
        commands: UnboundedSender<ScriptCmd>,
        push: UnboundedSender<mineral_protocol::Event>,
    ) -> Self {
        Self {
            events: Arc::new(Mutex::new(EventRegistry::default())),
            commands,
            push,
        }
    }
}

/// 把 `mineral.*` API 表挂进 VM 全局。须在 eval 用户脚本**之前**调用。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `host`: 宿主句柄(API 闭包捕获其克隆)
///
/// # Return:
///   挂表失败时为 `Err`(VM 级故障,调用方按 eval 失败同等处理)。
pub fn install_api(lua: &Lua, host: &ScriptHost) -> mlua::Result<()> {
    let mineral = lua.create_table()?;
    api::ui::install(lua, &mineral, host)?;
    api::events::install(lua, &mineral, host)?;
    api::player::install(lua, &mineral, host)?;
    api::observe::install(lua, &mineral, host)?;
    api::actions::install(lua, &mineral, host)?;
    lua.globals().set("mineral", mineral)
}
