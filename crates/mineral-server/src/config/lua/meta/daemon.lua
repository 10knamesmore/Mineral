---@meta mineral.daemon
-- daemon.lua 的 setup(api) 收到的宿主类型;音乐 API 只在 daemon 进程可用。

---@class mineral.DaemonApi
---@field sys mineral.Sys 当前 daemon 进程信息
---@field log mineral.Log
---@field player mineral.PlayerApi
---@field store mineral.StoreApi
---@field queue mineral.QueueApi
---@field library mineral.LibraryApi
---@field config mineral.DaemonConfigApi
---@field DEFER table 拦截回调的异步裁决标记
---@field hook fun(name: mineral.HookName, interceptor: fun(ctx: mineral.HookCtx): nil|boolean|mineral.HookReturn|table): nil
---@field download fun(song_id: string): nil
local mineral = {}

--- 拦截回调返回 DEFER 后,通过 ctx.resolve 补交裁决。
---@type table
mineral.DEFER = {}

--- 注册音乐拦截。nil/true 放行,false/skip 跳过,其他字段改写资源。
--- 同一拦截点按注册顺序执行,首个非放行结果短路。DEFER 用于异步音乐查询。
--- 即时播放/下载等待 script.hook_timeout_ms;预取等待预取窗口,超时放行。
---@param name mineral.HookName
---@param interceptor fun(ctx: mineral.HookCtx): nil|boolean|mineral.HookReturn|table
---@overload fun(name: "before_stream", interceptor: fun(ctx: mineral.BeforeStreamCtx): nil|boolean|mineral.HookReturn|table)
---@overload fun(name: "before_download", interceptor: fun(ctx: mineral.BeforeDownloadCtx): nil|boolean|mineral.HookReturn|table)
function mineral.hook(name, interceptor) end

--- 下载全限定 ID 的歌曲。
---@param song_id string namespace:value
function mineral.download(song_id) end

---@class mineral.PlayerApi
mineral.player = {}

function mineral.player.toggle() end
function mineral.player.next() end
function mineral.player.prev() end
function mineral.player.stop() end

--- 相对 seek(秒,可负)。
---@param secs number
function mineral.player.seek_rel(secs) end

--- 绝对 seek(秒,负数压回 0)。
---@param secs number
function mineral.player.seek_to(secs) end

--- 音量压到 0-100。
---@param pct integer
function mineral.player.set_volume(pct) end

---@param mode mineral.PlayMode
function mineral.player.set_mode(mode) end

--- 播放全限定 ID 的歌曲。
---@param song_id string namespace:value
function mineral.player.play(song_id) end

---@class mineral.StoreApi
mineral.store = {}

--- 异步读逐曲持久值;未命中为 nil,失败为 nil + 错误串。
---@param song_id string namespace:value
---@param key string 建议使用插件名前缀,如 plugin.rating
---@param on_value fun(value: mineral.StoreValue, err: string|nil): nil
function mineral.store.get(song_id, key, on_value) end

--- 写逐曲持久值;nil 删除。local_play_count/rating/last_played 为保留键,拒写。
---@param song_id string namespace:value
---@param key string
---@param value mineral.StoreValue
function mineral.store.set(song_id, key, value) end

---@class mineral.QueueApi
mineral.queue = {}

--- 异步读当前队列;数组顺序即队列顺序,失败为 nil + 错误串。
---@param on_songs fun(songs: mineral.Song[]|nil, err: string|nil): nil
function mineral.queue.list(on_songs) end

--- 按 id 重排或删减现有队列,允许重复现有 id;外来 id 使整次重排被拒。
---@param songs mineral.Song[]
function mineral.queue.set(songs) end

---@class mineral.LibraryApi
mineral.library = {}

--- 异步读经 curate_playlists 处理的聚合歌单快照,初始拉取期间等待各源就绪。
---@param on_playlists fun(playlists: mineral.PlaylistBrief[]|nil, err: string|nil): nil
function mineral.library.playlists(on_playlists) end

--- 异步读歌单条目,保留每项的来源 index;不是裸 Song 数组。
---@param playlist_id string namespace:value
---@param on_entries fun(entries: mineral.PlaylistEntry[]|nil, err: string|nil): nil
function mineral.library.tracks(playlist_id, on_entries) end

--- 异步搜索。source 省略时跨源聚合,单源失败跳过;指定无效来源则失败。
---@param query string
---@param opts? { source?: string, offset?: integer, limit?: integer } offset 默认 0,limit 默认 30
---@param on_songs fun(songs: mineral.Song[]|nil, err: string|nil): nil
---@overload fun(query: string, on_songs: fun(songs: mineral.Song[]|nil, err: string|nil): nil): nil
function mineral.library.search(query, opts, on_songs) end

--- 由歌曲来源的 playback provider 解析资源,失败为 nil + 错误串。
---@param song_id string namespace:value
---@param on_url fun(play_url: mineral.PlayUrl|nil, err: string|nil): nil
function mineral.library.song_url(song_id, on_url) end

--- 设置歌曲喜欢状态(本地持久化并提交到对应 channel)。
---@param song_id string namespace:value
---@param loved boolean
function mineral.library.love(song_id, loved) end

---@class mineral.DaemonConfigApi
mineral.config = {}

--- 当前 daemon 的 session 配置覆盖,重启即清,不修改 TUI 配置。
--- 偏表与点路径从 daemon.lua 根字段开始,再深合并和落型校验;错误路径/值被拒并记录告警。
--- 队列变换须随 daemon.lua 中的函数定义一起加载,不能覆盖 queue.transforms 描述表。
--- 字符串形式的 value = nil 撤销该路径覆盖,回落配置文件值。
---@param patch mineral.DaemonConfig 只写需要覆盖的 daemon 字段
---@overload fun(path: string, value: mineral.ConfigValue|nil)
function mineral.config.override(patch) end

return mineral
