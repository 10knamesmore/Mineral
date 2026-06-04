---@meta
-- Mineral host API 类型 stub(LuaCATS)。随程序分发,供编辑器补全 / 类型检查。
-- Phase 0:这些 API 在 TUI/CLI 进程为 no-op;daemon VM 落地后承载真实行为。
-- 不要 require 本文件,它只供 LSP 读取。

---@class mineral
mineral = {}

--- 订阅离散生命周期事件(必带 reason)。
---@param event "track_finished"|"download_completed"
---@param fn fun(...): nil
function mineral.on(event, fn) end

--- 注册具名动作(物理键解耦,多 client 共用触发面)。
---@param name string  动作注册名,如 "my.skip_short"
---@param fn fun(ctx: table): nil
function mineral.action(name, fn) end

--- 语法糖:匿名动作 + keys 表追加。
---@param key string
---@param fn fun(ctx: table): nil
function mineral.bind(key, fn) end

--- 订阅属性树变更(订阅即回放当前值;高频变化合并只回末值)。
---@param prop string  "player.song"|"player.state"|"player.volume"|"player.position"|"player.mode"|"queue.length"
---@param fn fun(value: any): nil
function mineral.observe(prop, fn) end

--- 读属性树当前值。
---@param prop string
---@return any
function mineral.get(prop) end

--- 下载指定歌曲。
---@param song_id string
function mineral.download(song_id) end

---@class mineral.player
mineral.player = {}

function mineral.player.toggle() end
function mineral.player.next() end
function mineral.player.prev() end
function mineral.player.stop() end

---@param secs number
function mineral.player.seek_rel(secs) end

---@param secs number
function mineral.player.seek_to(secs) end

---@param pct integer  0-100
function mineral.player.set_volume(pct) end

---@param mode string
function mineral.player.set_mode(mode) end

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
