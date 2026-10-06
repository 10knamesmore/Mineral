---@meta mineral.tui
-- tui.lua 的 setup(api) 收到的宿主类型;命令只影响当前 TUI,不在 daemon 执行。

---@class mineral.TuiApi
---@field sys mineral.Sys 当前 TUI 进程信息
---@field log mineral.Log
---@field ui mineral.TuiUiApi
---@field config mineral.TuiConfigApi
local mineral = {}

---@class mineral.TuiUiApi
mineral.ui = {}

--- toast ui inline
---@param msg any|(string|mineral.Span)[]
---@param opts? { kind?: "info"|"warn"|"error", id?: string, ttl_secs?: integer }
function mineral.ui.toast(msg, opts) end

--- toast ui card
---@param opts { title?: string|(string|mineral.Span)[], kind?: "info"|"warn"|"error", id?: string, ttl_secs?: integer, body: (string|(string|mineral.Span)[])[] }
function mineral.ui.card(opts) end

--- override the tui window title
---@param text string|nil
function mineral.ui.window_title(text) end

---@class mineral.TuiConfigApi
mineral.config = {}

--- override the runing tui config
---@param patch mineral.TuiConfig 只写需要覆盖的本地字段
---@overload fun(path: string, value: mineral.ConfigValue|nil)
function mineral.config.override(patch) end

return mineral
