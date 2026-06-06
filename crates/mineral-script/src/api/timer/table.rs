//! 定时器的承载结构:[`TimerTable`](注册 / 心跳收割 / handle 操作三方共享)
//! 与 Lua 侧 handle userdata。`after` / `every` 只是它的两个薄构造面。

use std::sync::Arc;
use std::time::{Duration, Instant};

use mlua::{Lua, Table};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::host::ScriptHost;

/// 定时器表:脚本 API 注册 / handle 操作 / 主循环心跳三方共享。
#[derive(Debug, Default)]
pub(crate) struct TimerTable {
    /// 自增 id 源。
    next: u64,

    /// 活跃定时器。
    entries: FxHashMap<u64, TimerEntry>,
}

/// 一只定时器的状态。
#[derive(Debug)]
struct TimerEntry {
    /// 回调在 VM 注册表里的句柄。
    callback: Arc<mlua::RegistryKey>,

    /// 触发间隔(`after` 的一次性延迟同存这里)。
    interval: Duration,

    /// 下次触发时刻;`None` = 暂停中(剩余计时冻结在 [`Self::remaining`])。
    deadline: Option<Instant>,

    /// 暂停时冻结的剩余时长(运行中无意义)。
    remaining: Duration,

    /// `true` = 周期触发(every);`false` = 一次性(after,触发后自动注销)。
    repeating: bool,
}

impl TimerTable {
    /// 挂入一只定时器,返回 handle 用的 id。
    fn insert(
        &mut self,
        callback: Arc<mlua::RegistryKey>,
        interval: Duration,
        repeating: bool,
    ) -> u64 {
        self.next = self.next.wrapping_add(1);
        self.entries.insert(
            self.next,
            TimerEntry {
                callback,
                interval,
                deadline: Some(Instant::now() + interval),
                remaining: interval,
                repeating,
            },
        );
        self.next
    }

    /// 最近一只运行中定时器的到期时刻(主循环据此定 `recv_timeout`);
    /// 无运行中定时器为 `None`(主循环长等消息)。
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.entries.values().filter_map(|e| e.deadline).min()
    }

    /// 收割到期定时器:返回要调的回调列表;周期项顺延,一次性项注销。
    ///
    /// 只改表,不调回调 —— 调用方锁外执行(回调里再操作 timer 不撞锁)。
    pub(crate) fn collect_due(&mut self, now: Instant) -> Vec<Arc<mlua::RegistryKey>> {
        let due_ids = self
            .entries
            .iter()
            .filter(|(_, e)| e.deadline.is_some_and(|d| d <= now))
            .map(|(id, _)| *id)
            .collect::<Vec<u64>>();
        let mut callbacks = Vec::with_capacity(due_ids.len());
        for id in due_ids {
            if let Some(entry) = self.entries.get_mut(&id) {
                callbacks.push(Arc::clone(&entry.callback));
                if entry.repeating {
                    entry.deadline = Some(now + entry.interval);
                } else {
                    self.entries.remove(&id);
                }
            }
        }
        callbacks
    }

    /// 暂停:冻结剩余计时(已暂停 / 已注销则 no-op)。
    fn stop(&mut self, id: u64, now: Instant) {
        if let Some(entry) = self.entries.get_mut(&id)
            && let Some(deadline) = entry.deadline.take()
        {
            entry.remaining = deadline.saturating_duration_since(now);
        }
    }

    /// 续跑:从冻结的剩余计时处继续(运行中 / 已注销则 no-op)。
    fn resume(&mut self, id: u64, now: Instant) {
        if let Some(entry) = self.entries.get_mut(&id)
            && entry.deadline.is_none()
        {
            entry.deadline = Some(now + entry.remaining);
        }
    }

    /// 注销(幂等)。
    fn kill(&mut self, id: u64) {
        self.entries.remove(&id);
    }
}

/// Lua 侧的定时器句柄(userdata):`t:stop()` / `t:resume()` / `t:kill()`。
struct TimerHandle {
    /// 表内 id。
    id: u64,

    /// 共享定时器表。
    timers: Arc<Mutex<TimerTable>>,
}

impl mlua::UserData for TimerHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("stop", |_lua, this, ()| {
            this.timers.lock().stop(this.id, Instant::now());
            Ok(())
        });
        methods.add_method("resume", |_lua, this, ()| {
            this.timers.lock().resume(this.id, Instant::now());
            Ok(())
        });
        methods.add_method("kill", |_lua, this, ()| {
            this.timers.lock().kill(this.id);
            Ok(())
        });
    }
}

/// 挂一个构造器(`after` / `every` 同构):`(ms, fn) -> handle`。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `timer`: `mineral.timer` 子表
///   - `host`: 宿主句柄(共享定时器表)
///   - `name`: Lua 函数名
///   - `repeating`: `true` = every,`false` = after
pub(crate) fn install_ctor(
    lua: &Lua,
    timer: &Table,
    host: &ScriptHost,
    name: &str,
    repeating: bool,
) -> mlua::Result<()> {
    let timers = Arc::clone(&host.timers);
    timer.set(
        name,
        lua.create_function(move |lua, (ms, callback): (u64, mlua::Function)| {
            let key = Arc::new(lua.create_registry_value(callback)?);
            let id = timers
                .lock()
                .insert(key, Duration::from_millis(ms), repeating);
            Ok(TimerHandle {
                id,
                timers: Arc::clone(&timers),
            })
        })?,
    )
}
