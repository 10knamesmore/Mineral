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
    /// `track_started` 回调。
    pub(crate) track_started: Vec<Arc<mlua::RegistryKey>>,

    /// `track_finished` 回调。
    pub(crate) track_finished: Vec<Arc<mlua::RegistryKey>>,

    /// `download_completed` 回调。
    pub(crate) download_completed: Vec<Arc<mlua::RegistryKey>>,

    /// 属性观察者(`mineral.observe`,按属性键分桶)。
    pub(crate) observers: FxHashMap<PropKey, Vec<Arc<mlua::RegistryKey>>>,

    /// 同步拦截 hook(`mineral.hook`,按拦截点分桶;注册顺序调用,首个
    /// 非放行裁决短路生效)。
    pub(crate) hooks: FxHashMap<crate::hooks::HookKind, Vec<Arc<mlua::RegistryKey>>>,

    /// 自定义总线订阅者(`mineral.on_message`,按消息名分桶)。
    pub(crate) bus_subs: FxHashMap<String, Vec<Arc<mlua::RegistryKey>>>,

    /// 属性当前值缓存:daemon 每次投递 `PropertyChanged` 时更新;
    /// `observe` 注册时有值即回放、`mineral.get` 同源读。
    pub(crate) props: FxHashMap<PropKey, PropValue>,

    /// 具名动作(`mineral.action` 注册;`mineral.bind` 的匿名 fn 以内部名进表)。
    pub(crate) actions: FxHashMap<String, Arc<mlua::RegistryKey>>,

    /// `mineral.bind` 产生的键绑定表(注册顺序;client 经 `ScriptBinds` 拉取)。
    pub(crate) binds: Vec<mineral_protocol::ScriptBind>,

    /// bind 内部名计数器([`Self::next_bind_name`] 用)。
    next_bind: u64,

    /// spawn 标识计数器([`Self::next_spawn_id`] 用)。
    next_spawn: u64,
}

impl EventRegistry {
    /// 生成下一个 bind 匿名动作的内部名(`bind#1` 起,与用户 action 名隔开)。
    pub(crate) fn next_bind_name(&mut self) -> String {
        self.next_bind = self.next_bind.wrapping_add(1);
        format!("bind#{}", self.next_bind)
    }

    /// 分配下一个 spawn 标识(`handle:kill()` 路由用)。
    pub(crate) fn next_spawn_id(&mut self) -> crate::proc::SpawnId {
        self.next_spawn = self.next_spawn.wrapping_add(1);
        crate::proc::SpawnId(self.next_spawn)
    }
}

/// 在途异步查询表:查询类 API(`store.get` 等)把 Lua 回调挂在这里,
/// daemon 泵以 [`crate::QueryId`] 回投结果时取出调用(取出即删,一次性)。
#[derive(Debug, Default)]
pub(crate) struct PendingQueries {
    /// 自增 id 源。
    next: u64,

    /// 在途查询:id → Lua 回调。
    map: FxHashMap<u64, Arc<mlua::RegistryKey>>,
}

impl PendingQueries {
    /// 挂入一个回调,返回回投句柄。
    pub(crate) fn insert(&mut self, callback: Arc<mlua::RegistryKey>) -> crate::QueryId {
        self.next = self.next.wrapping_add(1);
        self.map.insert(self.next, callback);
        crate::QueryId(self.next)
    }

    /// 取出(并移除)一个在途回调;重复 / 未知 id 为 `None`。
    pub(crate) fn take(&mut self, query: crate::QueryId) -> Option<Arc<mlua::RegistryKey>> {
        self.map.remove(&query.0)
    }
}

/// 脚本宿主句柄:Lua API 闭包与 dispatch 层共享的全部可变面。
#[derive(Clone, Debug)]
pub struct ScriptHost {
    /// 事件回调注册表。
    pub(crate) events: Arc<Mutex<EventRegistry>>,

    /// 在途异步查询表(查询类 API 的回调中转)。
    pub(crate) pending: Arc<Mutex<PendingQueries>>,

    /// 定时器表(`mineral.timer.*` 注册,主循环心跳收割)。
    pub(crate) timers: Arc<Mutex<crate::api::timer::table::TimerTable>>,

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
            pending: Arc::new(Mutex::new(PendingQueries::default())),
            timers: Arc::new(Mutex::new(crate::api::timer::table::TimerTable::default())),
            commands,
            push,
        }
    }

    /// 播种属性缓存(热重载起新 VM 前,entries 取 daemon 侧当前值)。
    ///
    /// 须在 eval 用户脚本**之前**调用:`observe` 注册时的回放与顶层
    /// `mineral.get` 读的都是这份缓存,而 daemon 只下发 diff —— 不播种的话
    /// 新 VM 要等属性下次真变更才恢复,部分属性可能永不再变。
    ///
    /// # Params:
    ///   - `entries`: 属性当前值快照(重复键后写赢)
    pub fn seed_props(&self, entries: Vec<(PropKey, PropValue)>) {
        let mut registry = self.events.lock();
        for (key, value) in entries {
            registry.props.insert(key, value);
        }
    }

    /// 把一个 Lua 回调挂入在途查询表,返回随命令带出的回投句柄。
    ///
    /// # Params:
    ///   - `lua`: 持有回调的 VM
    ///   - `callback`: 查询完成时要调的 Lua 函数
    pub(crate) fn register_query(
        &self,
        lua: &Lua,
        callback: mlua::Function,
    ) -> mlua::Result<crate::QueryId> {
        let key = Arc::new(lua.create_registry_value(callback)?);
        Ok(self.pending.lock().insert(key))
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

    // 顶层函数(与 api/ 顶层文件一一对应)。
    api::on::install(lua, &mineral, host)?;
    api::hook::install(lua, &mineral, host)?;
    api::action::install(lua, &mineral, host)?;
    api::bind::install(lua, &mineral, host)?;
    api::observe::install(lua, &mineral, host)?;
    api::get::install(lua, &mineral, host)?;
    api::download::install(lua, &mineral, host)?;
    api::spawn::install(lua, &mineral, host)?;
    api::emit::install(lua, &mineral, host)?;
    api::on_message::install(lua, &mineral, host)?;

    // 子表(与 api/ 子目录一一对应;各目录根的 install 内部再分发到单函数文件)。
    api::player::install(lua, &mineral, host)?;
    api::ui::install(lua, &mineral, host)?;
    api::log::install(lua, &mineral)?;
    api::sys::install(lua, &mineral)?;
    api::store::install(lua, &mineral, host)?;
    api::queue::install(lua, &mineral, host)?;
    api::library::install(lua, &mineral, host)?;
    api::timer::install(lua, &mineral, host)?;

    lua.globals().set("mineral", mineral)
}

#[cfg(test)]
mod tests {
    use crate::api::test_support::vm_with_host;
    use crate::message::{PropKey, PropValue};

    /// 热重载播种:seed 后 observe 注册立即回放、顶层 get 同源可读 ——
    /// 新 VM 不必等 daemon 下一次属性真变更(diff 下发可能永不再来)。
    #[test]
    fn seeded_props_replay_to_observe_and_get() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        host.seed_props(vec![
            (PropKey::PlayerVolume, PropValue::Int(42)),
            (PropKey::PlayerState, PropValue::Str("stopped".to_owned())),
        ]);
        lua.load(
            r#"
            assert(mineral.get("player.volume") == 42, "get 必须读到播种值")
            seen = nil
            mineral.observe("player.state", function(v) seen = v end)
            assert(seen == "stopped", "observe 注册必须回放播种值")
            "#,
        )
        .exec()?;
        Ok(())
    }
}
