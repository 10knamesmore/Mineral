---@meta
-- Mineral host API 类型 stub(LuaCATS)。随程序分发,供编辑器补全 / 类型检查。
-- Phase 0:这些 API 在 TUI/CLI 进程为 no-op;daemon VM 落地后承载真实行为。
-- 不要 require 本文件,它只供 LSP 读取。

---@class mineral
mineral = {}

--- 事件回调里的歌曲投影(最小字段集;完整归一化投影是 sub05 的事)。
---@class mineral.Song
---@field id string  全局唯一 id(`namespace:value`,如 "netease:123"),可直接回喂 player API
---@field title string  歌名
---@field duration_ms integer  时长(毫秒),拿不到为 0

--- 曲目结束原因(与 Rust `TrackFinishedReason` 由守卫测试钉死同步)。
---@alias mineral.FinishReason "eof"|"skip"|"error"|"stop"

--- `track_finished` 回调的 args。
---@class mineral.TrackFinishedArgs
---@field song mineral.Song  结束的歌
---@field reason mineral.FinishReason  结束原因

--- `download_completed` 回调的 args。
---@class mineral.DownloadCompletedArgs
---@field song mineral.Song  下载完成的歌
---@field path string  落盘路径

--- `mineral.on` 的合法事件名(字符串枚举;与 Rust 事件墙由守卫测试钉死同步)。
---@alias mineral.EventName "track_finished"|"download_completed"

--- 订阅离散生命周期事件。回调统一收单一 args table(nvim autocmd 风格,
--- 以后加字段零破坏);按事件名字面量分派出对应的 args 类型(LuaLS 走主签名
--- 兜底时 args 为 union,字段补全给并集)。
---@param event mineral.EventName
---@param fn fun(args: mineral.TrackFinishedArgs|mineral.DownloadCompletedArgs): nil
---@overload fun(event: "track_finished", fn: fun(args: mineral.TrackFinishedArgs))
---@overload fun(event: "download_completed", fn: fun(args: mineral.DownloadCompletedArgs))
function mineral.on(event, fn) end

--- 注册具名动作(物理键解耦,多 client 共用触发面)。重名 / 空名报错。
---@param name string  动作注册名,如 "my.skip_short"
---@param fn fun(ctx: table): nil
function mineral.action(name, fn) end

--- 语法糖:匿名动作 + keys 表追加。**尚未实现**:当前调用只记一条
--- warn 日志并被忽略,不注册任何东西。
---@param key string
---@param fn fun(ctx: table): nil
function mineral.bind(key, fn) end

--- 可观测属性名(字符串枚举;与 Rust `PropKey` 由守卫测试钉死同步)。
---@alias mineral.PropName "player.song"|"player.state"|"player.volume"|"player.position"|"player.mode"|"queue.length"

--- 播放循环模式的蛇形稳定名(与 Rust `PlayMode::script_name` 由守卫测试钉死同步)。
---@alias mineral.PlayMode "sequential"|"shuffle"|"repeat_all"|"repeat_one"

--- 播放态。
---@alias mineral.PlayerState "playing"|"paused"|"stopped"

--- 订阅属性树变更(订阅即回放当前值;高频变化合并只回末值)。
--- 回调收裸值;按属性名字面量分派出对应的值类型。
---@param prop mineral.PropName
---@param fn fun(value: any): nil
---@overload fun(prop: "player.song", fn: fun(value: string|nil))
---@overload fun(prop: "player.state", fn: fun(value: mineral.PlayerState))
---@overload fun(prop: "player.volume", fn: fun(value: integer))
---@overload fun(prop: "player.position", fn: fun(value: integer))
---@overload fun(prop: "player.mode", fn: fun(value: mineral.PlayMode))
---@overload fun(prop: "queue.length", fn: fun(value: integer))
function mineral.observe(prop, fn) end

--- 读属性树当前值(daemon 尚未推送过该属性时为 nil)。
---@param prop mineral.PropName
---@return any
---@overload fun(prop: "player.song"): string|nil
---@overload fun(prop: "player.state"): mineral.PlayerState|nil
---@overload fun(prop: "player.volume"): integer|nil
---@overload fun(prop: "player.position"): integer|nil
---@overload fun(prop: "player.mode"): mineral.PlayMode|nil
---@overload fun(prop: "queue.length"): integer|nil
function mineral.get(prop) end

--- 下载指定歌曲(id 用 `namespace:value` 全限定形式,如 "netease:123")。
---@param song_id string
function mineral.download(song_id) end

---@class mineral.player
mineral.player = {}

function mineral.player.toggle() end
function mineral.player.next() end
function mineral.player.prev() end
function mineral.player.stop() end

--- 相对 seek(秒,可负)。
---@param secs number
function mineral.player.seek_rel(secs) end

--- 绝对 seek(秒;负数压回 0)。
---@param secs number
function mineral.player.seek_to(secs) end

--- 设音量(越界 clamp 到 0-100,不报错)。
---@param pct integer  0-100
function mineral.player.set_volume(pct) end

--- 设播放模式(未知名报错)。
---@param mode mineral.PlayMode
function mineral.player.set_mode(mode) end

--- 播放指定歌曲(id 用 `namespace:value` 全限定形式,如 "netease:123")。
---@param song_id string
function mineral.player.play(song_id) end

---@class mineral.ui
mineral.ui = {}

--- 推送 toast 到 client(同 id 替换不堆叠)。
---@param msg string
---@param opts? { kind?: "info"|"warn"|"error", id?: string }
function mineral.ui.toast(msg, opts) end

---@class mineral.log
mineral.log = {}

---@param msg string
function mineral.log.info(msg) end

---@param msg string
function mineral.log.warn(msg) end

return mineral
