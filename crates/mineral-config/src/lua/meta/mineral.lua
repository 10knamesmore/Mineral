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

--- 同步拦截点名(与 Rust `HookKind` 由守卫测试钉死同步)。
---@alias mineral.HookName "before_play"|"before_download"

--- 同步拦截回调的 ctx。
---@class mineral.HookCtx
---@field song mineral.Song  触发拦截的歌
---@field url string  原始播放 / 下载 URL
---@field quality string  原始音质名(standard/higher/exhigh/lossless/hires)
---@field kind mineral.HookName  拦截点名

--- 同步拦截回调的改写返回值(字段全可选,只给要改的)。
---@class mineral.HookReturn
---@field url? string  改写后的 URL
---@field quality? string  改写后的音质名
---@field skip? string  跳过本次,值为原因(给了 skip 则忽略 url/quality)

--- 注册同步拦截 hook:daemon 在起播前(`before_play`)/ 下载写盘前
--- (`before_download`)同步等待回调裁决。返回值:`nil`(或 `true`)放行;
--- `false` 或 `{ skip = "原因" }` 跳过本次(播放跳下一首 / 下载记 skip);
--- `{ url = ?, quality = ? }` 改写后继续。改写过的播放流不入缓存。
--- **回调要快**:超过 `script.hook_timeout_ms`(默认 2000ms)按放行处理。
--- 同一拦截点可注册多个,按注册顺序调用,首个非放行返回值短路生效。
---@param name mineral.HookName
---@param fn fun(ctx: mineral.HookCtx): nil|boolean|mineral.HookReturn
function mineral.hook(name, fn) end

--- 子进程句柄(`mineral.spawn` 返回)。
---@class mineral.SpawnHandle
local SpawnHandle = {}

--- 中止子进程(SIGKILL;已退出 no-op)。
function SpawnHandle:kill() end

--- 子进程结束后的结果(`mineral.spawn` 回调入参)。
---@class mineral.SpawnResult
---@field code? integer  退出码;被信号终止(含 kill)时为 nil
---@field stdout string  标准输出
---@field stderr string  标准错误
---@field killed boolean  是否被 `handle:kill()` 中止

--- 起一个异步子进程,退出后回调 `fn(result, nil)`;spawn 本身失败
--- (可执行不存在 / 超并发上限 `script.spawn_max_concurrent`)回调收
--- `(nil, err)`。`args` 是字符串数组(首元素为可执行文件),不经 shell。
---@param args string[]  命令与参数,如 `{"curl", "-s", url}`
---@param opts? { cwd?: string, env?: table<string, string> }  工作目录 / 环境变量
---@param fn fun(result: mineral.SpawnResult|nil, err: string|nil): nil
---@return mineral.SpawnHandle handle
---@overload fun(args: string[], fn: fun(result: mineral.SpawnResult|nil, err: string|nil): nil): mineral.SpawnHandle
function mineral.spawn(args, opts, fn) end

--- 总线载荷:标量与嵌套 table(数组形或字符串键映射,可混嵌套不可同层混用;
--- 不支持 function/userdata,嵌套上限 8 层)。
---@alias mineral.BusPayload nil|boolean|number|string|table

--- 发一条自定义总线消息:本 VM 的 `on_message` 订阅者同步收到,
--- 订阅 Bus 类别的外部 client 经 daemon 原样转发收到(daemon 零解释)。
--- 命名建议 `插件名.事件` 形,避免与他人脚本撞名。
---@param name string  消息名
---@param payload? mineral.BusPayload  载荷
function mineral.emit(name, payload) end

--- 订阅自定义总线消息(按名精确匹配;同名可多订,注册顺序调用)。
---@param name string  消息名
---@param fn fun(payload: mineral.BusPayload): nil
function mineral.on_message(name, fn) end

--- 按键瞬间 client 正在展示的视图(与 Rust `ViewKind` 由守卫测试钉死同步)。
---@alias mineral.ViewKind "playlists"|"tracks"|"queue"|"fullscreen"|"search"

--- 选中歌单的轻量引用。
---@class mineral.PlaylistRef
---@field id string  歌单 id(`namespace:value`)
---@field name string  歌单名

--- 动作回调收到的上下文(按键瞬间采集;CLI 等无界面触发面为空表,字段全 nil)。
--- 只携带 daemon 不知道的 client 侧信息;播放器态(音量/进度/队列)用 `mineral.get`。
---@class mineral.ActionCtx
---@field view mineral.ViewKind|nil  按键时所在视图
---@field selected_song mineral.Song|nil  列表光标选中的歌(队列浮层取光标条目)
---@field selected_playlist mineral.PlaylistRef|nil  选中 / 所在的歌单
---@field now_playing mineral.Song|nil  在播的歌(停止态 nil)
---@field selected_loved boolean|nil  选中歌的 ♥ 态(无选中 / 未知为 nil)
---@field search_query string|nil  当前搜索 / 过滤词(空词为 nil)

--- 注册具名动作(物理键解耦,多 client 共用触发面)。重名 / 空名报错。
--- 触发面:TUI `tui.keys.script` 绑键(ctx 带按键上下文)/ CLI `mineral action <name>`(ctx 空表)。
---@param name string  动作注册名,如 "my.skip_short"
---@param fn fun(ctx: mineral.ActionCtx): nil
function mineral.action(name, fn) end

--- 语法糖:匿名动作 + 键位一步绑定(= `mineral.action(内部名, fn)` +
--- 键合进 TUI keymap)。键字符串文法与 `tui.keys` 一致(nvim 表示法,如 "X" / "<C-g>");
--- 非法键名在 TUI 侧 warn 跳过,不影响其余绑定。
---@param key string  键字符串,如 "X" / "<C-g>"
---@param fn fun(ctx: mineral.ActionCtx): nil
function mineral.bind(key, fn) end

--- 可观测属性名(字符串枚举;与 Rust `PropKey` 由守卫测试钉死同步)。
---@alias mineral.PropName "player.song"|"player.state"|"player.volume"|"player.position"|"player.mode"|"queue.length"|"terminal"

--- 终端 UI 状态(`terminal` 复合属性的值;无 client 在线时整体为 nil)。
---@class mineral.TerminalState
---@field rows integer  终端行数
---@field cols integer  终端列数
---@field fullscreen boolean  是否处于全屏播放态

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
---@overload fun(prop: "terminal", fn: fun(value: mineral.TerminalState|nil))
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
---@overload fun(prop: "terminal"): mineral.TerminalState|nil
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

--- per-song 持久 KV 的标量值(`nil` 写入 = 删除该 key)。
---@alias mineral.StoreValue integer|number|string|boolean|nil

---@class mineral.store
mineral.store = {}

--- 读 per-song 持久值(回调风格,不阻塞脚本线程)。
--- 成功 `fn(值, nil)`(未命中值为 nil);失败 `fn(nil, 错误串)`。
---@param song_id string  歌曲 id(`namespace:value` 全限定形式)
---@param key string  开放键(建议带 `.` 前缀,如 "plugin.skipcount")
---@param fn fun(value: mineral.StoreValue, err: string|nil): nil
function mineral.store.get(song_id, key, fn) end

--- 写 per-song 持久值(fire-and-forget;`nil` 删除该 key)。
--- 保留键(`local_play_count` / `rating` / `last_played`)拒写。
---@param song_id string
---@param key string
---@param value mineral.StoreValue
function mineral.store.set(song_id, key, value) end

--- per-song 数值自增(key 不存在以 delta 起步;现有值非整数报错)。
--- 带回调时 `fn(自增后的值, nil)` / `fn(nil, 错误串)`。
---@param song_id string
---@param key string
---@param delta integer  增量(可负)
---@param fn? fun(value: integer|nil, err: string|nil): nil
function mineral.store.inc(song_id, key, delta, fn) end

---@class mineral.queue
mineral.queue = {}

--- 读当前播放队列(回调风格;数组顺序即队列顺序)。
--- 跳播用 `mineral.player.play(song.id)`。队列编辑是规划中的能力,本期只读。
---@param fn fun(songs: mineral.Song[], err: string|nil): nil
function mineral.queue.list(fn) end

--- 歌单的轻量投影(`library.playlists` 出参;曲目另经 `library.tracks` 拉)。
---@class mineral.PlaylistBrief
---@field id string  歌单 id(`namespace:value`)
---@field name string  歌单名
---@field track_count integer  曲目数

---@class mineral.library
mineral.library = {}

--- 读用户歌单列表(跨源聚合;某源拉取失败跳过该源,不整体失败)。
---@param fn fun(playlists: mineral.PlaylistBrief[], err: string|nil): nil
function mineral.library.playlists(fn) end

--- 读指定歌单的曲目。
---@param playlist_id string  歌单 id(`namespace:value`)
---@param fn fun(songs: mineral.Song[], err: string|nil): nil
function mineral.library.tracks(playlist_id, fn) end

--- 按关键词搜索歌曲(异步回调)。
--- `opts.source` 省略 = 跨全部源聚合(单源失败跳过该源);
--- 指定则只搜该源,无对应源时回调收 `(nil, err)`。
---@param query string  关键词
---@param opts? { source?: string, offset?: integer, limit?: integer }  搜索选项(offset 默认 0,limit 默认 30)
---@param fn fun(songs: mineral.Song[]|nil, err: string|nil): nil
---@overload fun(query: string, fn: fun(songs: mineral.Song[]|nil, err: string|nil): nil): nil
function mineral.library.search(query, opts, fn) end

--- 设/取消一首歌的 love(♥)。fire-and-forget(本地 persist + 远端)。
---@param song_id string
---@param loved boolean
function mineral.library.love(song_id, loved) end

--- 定时器句柄(`timer.after` / `timer.every` 返回)。
---@class mineral.Timer
local Timer = {}

--- 暂停:冻结剩余计时(已暂停 / 已注销 no-op)。
function Timer:stop() end

--- 续跑:从冻结的剩余计时处继续。
function Timer:resume() end

--- 注销(幂等;一次性 `after` 触发后自动注销)。
function Timer:kill() end

---@class mineral.timer
mineral.timer = {}

--- 一次性定时器:`ms` 毫秒后触发一次(回调与事件回调同受看门狗保护)。
---@param ms integer
---@param fn fun(): nil
---@return mineral.Timer
function mineral.timer.after(ms, fn) end

--- 周期定时器:每 `ms` 毫秒触发(慢回调不会重入 —— 脚本线程串行)。
---@param ms integer
---@param fn fun(): nil
---@return mineral.Timer
function mineral.timer.every(ms, fn) end

---@class mineral.ui
mineral.ui = {}

--- 推送 toast 到 client(同 id 替换不堆叠)。
--- msg 是 `print` 式宽容:任意值经 tostring 显示;**nil 静默跳过**
--- (`toast(ctx.search_query)` 这类可空链无词时安静,不报错)。
---@param msg any  显示内容(nil 跳过;非字符串经 tostring)
---@param opts? { kind?: "info"|"warn"|"error", id?: string, ttl_secs?: integer }  ttl_secs 缺省用 client 配置(toast.flash_ttl_secs)
function mineral.ui.toast(msg, opts) end

--- session 级 UI 旋钮覆盖(daemon 重启即清,不写配置文件)。
--- key 约定 = 配置路径(如 "lyrics.fullscreen_line_gap" / "lyrics.compact_line_gap");
--- daemon 零解释转发,未知 key 由 client 边缘 warn + 丢。
--- `value = nil` 撤销覆盖,client 回落自己的配置值。
---@param key string  旋钮键,如 "lyrics.fullscreen_line_gap"
---@param value mineral.BusPayload|nil  覆盖值;nil = 撤销
function mineral.ui.override(key, value) end

---@class mineral.log
mineral.log = {}

---@param msg string
function mineral.log.info(msg) end

---@param msg string
function mineral.log.warn(msg) end

return mineral
