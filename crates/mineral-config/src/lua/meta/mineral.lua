---@meta
-- Mineral host API 类型。随程序分发,供编辑器补全 / 类型检查。

---@class mineral
mineral = {}

--- 歌曲信息
---@class mineral.Song
---@field id string  全局 ID（namespace:value）
---@field title string  歌名
---@field duration_ms integer|nil  曲目时长
---@field artists string[]  艺人名，主艺人在前
---@field album string|nil  专辑名
---@field cover_url string|nil  封面 URL 或本地路径
---@field source_url string|nil  原始资源路径或 URL
---@field source string  来源名
---@field url string|nil  网页分享链接

--- 歌单信息
---@class mineral.Playlist
---@field id string  全局 ID（namespace:value）
---@field name string  歌单名
---@field description string  简介，缺失为空串
---@field track_count integer  总曲目数，可能多于已加载曲目
---@field cover_url string|nil  封面 URL
---@field source string  来源名
---@field url string|nil  网页分享链接
---@field songs mineral.Song[]  已加载曲目，可能未加载完整

--- 专辑信息
---@class mineral.Album
---@field id string  全局 ID（namespace:value）
---@field name string  专辑名
---@field artists string[]  艺人名，主艺人在前
---@field description string  简介，缺失为空串
---@field track_count integer|nil  总曲目数，未知为 nil
---@field cover_url string|nil  封面 URL
---@field source string  来源名
---@field url string|nil  网页分享链接
---@field songs mineral.Song[]  已加载曲目，可能未加载完整

--- 艺人信息
---@class mineral.Artist
---@field id string  全局 ID（namespace:value）
---@field name string  艺名
---@field description string  简介，缺失为空串
---@field follower_count integer|nil  关注者数
---@field album_count integer|nil  专辑数
---@field song_count integer|nil  歌曲数
---@field avatar_url string|nil  头像 URL
---@field source string  来源名
---@field url string|nil  网页分享链接
---@field songs mineral.Song[]  代表曲，未加载时为空数组

--- 曲目结束原因
---@alias mineral.FinishReason "eof"|"skip"|"error"|"stop"

--- `track_started` 回调的 args。
---@class mineral.TrackStartedArgs
---@field song mineral.Song  开始播放的歌

--- `track_finished` 回调的 args。
---@class mineral.TrackFinishedArgs
---@field song mineral.Song  结束的歌
---@field reason mineral.FinishReason  结束原因

--- `download_completed` 回调的 args。
---@class mineral.DownloadCompletedArgs
---@field song mineral.Song  下载完成的歌
---@field path string  落盘路径
---@field quality "standard"|"higher"|"exhigh"|"lossless"|"hires"  下载请求档位；hook 可声明替代资源的目录档位
---@field format string|nil  容器格式

--- `mineral.on` 的合法事件名(字符串枚举;与 Rust 事件墙由守卫测试钉死同步)。
---@alias mineral.EventName "track_started"|"track_finished"|"download_completed"

--- 订阅离散生命周期事件。回调统一收单一 args table(nvim autocmd 风格,
--- 以后加字段零破坏);按事件名字面量分派出对应的 args 类型(LuaLS 走主签名
--- 兜底时 args 为 union,字段补全给并集)。
--- `track_started` = 在播曲目变更(远端起播 / 本地命中 / gapless 推进全覆盖;
--- 同曲重启 / 单曲循环不重复触发)——切歌通知等观察类需求用它,别用 hook。
---@param event mineral.EventName
---@param handler fun(args: mineral.TrackStartedArgs|mineral.TrackFinishedArgs|mineral.DownloadCompletedArgs): nil
---@overload fun(event: "track_started", handler: fun(args: mineral.TrackStartedArgs))
---@overload fun(event: "track_finished", handler: fun(args: mineral.TrackFinishedArgs))
---@overload fun(event: "download_completed", handler: fun(args: mineral.DownloadCompletedArgs))
function mineral.on(event, handler) end

--- 同步拦截点名(与 Rust `HookKind` 由守卫测试钉死同步)。
---@alias mineral.HookName "before_stream"|"before_download"

--- DEFER 哨兵:拦截回调返回它 = 裁决稍后经 `ctx.resolve(...)` 补交
--- (异步场景:回调里发起 `mineral.library.search`,在其回调里 resolve)。
---@type table
mineral.DEFER = {}

--- 拦截回调共用上下文
---@class mineral.HookCtx
---@field song mineral.Song  触发拦截的歌
---@field kind mineral.HookName  拦截点名
---@field resolve fun(decision: nil|boolean|mineral.HookReturn): nil  延迟补交裁决(配合返回 DEFER);只认第一次,超时后补交静默丢
local HookCtx = {}

--- 播放拦截上下文
---@class mineral.BeforeStreamCtx: mineral.HookCtx
---@field mode "immediate"|"prefetch"  即时起播或预取；等待上限分别为 script.hook_timeout_ms、曲尾预取窗口
---@field url string|nil  播放 URL，解析失败为 nil
---@field quality string  请求音质，无 URL 时仍有值
---@field unplayable boolean  是否无可播 URL
local BeforeStreamCtx = {}

--- 下载拦截上下文
---@class mineral.BeforeDownloadCtx: mineral.HookCtx
---@field url string|nil  下载直链，解析失败为 nil
---@field quality string  请求音质，无直链时仍有值
---@field unplayable boolean  是否无下载直链
local BeforeDownloadCtx = {}

--- 拦截改写结果，只填要改的字段
---@class mineral.HookReturn
---@field url? string  改写后的 URL
---@field quality? string  改写后的音质名
---@field headers? string[][]  请求头，{{name, value}} 数组
---@field layout? "contiguous"|"chunked"  容器布局；分片用 chunked，直链用 contiguous；改 URL 时省略为 chunked
---@field bitrate_bps? integer  实际码率，仅供显示
---@field format? string  容器格式，仅供显示
---@field skip? string  跳过原因，优先于其他字段

--- 注册同步拦截 hook:daemon 在歌走向「开播」的提交点(`before_stream`,即时起播
--- 前 / gapless 预取武装前,见 `ctx.mode`)/ 下载写盘前(`before_download`)等待
--- 回调裁决。
---
--- 返回值契约:
--- - `nil` 或 `true` —— 放行,原样继续
--- - `false` 或 `{ skip = "原因" }` —— 跳过本次(即时口推进下一首 /
---   预取口否决预排、队列不动 / 下载记 skip)
--- - `{ url = ?, quality = ?, ... }` —— 改写后继续(字段见 `mineral.HookReturn`;
---   改写过的播放流不入缓存)
--- - `mineral.DEFER` —— 裁决稍后经 `ctx.resolve(...)` 补交(异步搜索期间
---   不阻塞脚本线程)
---
--- 注意:
--- - **同步返回要快**:超过预算(immediate = `script.hook_timeout_ms`,默认
---   2000ms;prefetch = 预取窗口)按放行处理;DEFER 后忘记 resolve 同样超时放行
--- - 同一拦截点可注册多个,按注册顺序调用,首个非放行返回值(或 DEFER)短路生效
---@param name mineral.HookName
---@param interceptor fun(ctx: mineral.HookCtx): nil|boolean|mineral.HookReturn|table
---@overload fun(name: "before_stream", interceptor: fun(ctx: mineral.BeforeStreamCtx): nil|boolean|mineral.HookReturn|table)
---@overload fun(name: "before_download", interceptor: fun(ctx: mineral.BeforeDownloadCtx): nil|boolean|mineral.HookReturn|table)
function mineral.hook(name, interceptor) end

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

--- 起一个异步子进程,退出后回调 `on_exit(result, nil)`;spawn 本身失败
--- (可执行不存在 / 超并发上限 `script.spawn_max_concurrent`)回调收
--- `(nil, err)`。`args` 是字符串数组(首元素为可执行文件),不经 shell。
---@param args string[]  命令与参数,如 `{"curl", "-s", url}`
---@param opts? { cwd?: string, env?: table<string, string> }  工作目录 / 环境变量
---@param on_exit fun(result: mineral.SpawnResult|nil, err: string|nil): nil
---@return mineral.SpawnHandle handle
---@overload fun(args: string[], on_exit: fun(result: mineral.SpawnResult|nil, err: string|nil): nil): mineral.SpawnHandle
function mineral.spawn(args, opts, on_exit) end

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
---@param handler fun(payload: mineral.BusPayload): nil
function mineral.on_message(name, handler) end

--- 按键时所在视图
---@alias mineral.ViewKind "playlists"|"tracks"|"queue"|"fullscreen"|"search"

--- 选中歌单的轻量引用。
---@class mineral.PlaylistRef
---@field id string  全局 ID（namespace:value）
---@field name string  歌单名

--- 动作上下文，界面字段在无界面触发时为 nil
---@class mineral.ActionCtx
---@field view mineral.ViewKind|nil  按键时所在视图
---@field selected_song mineral.Song|nil  光标选中的歌曲
---@field selected_playlist mineral.PlaylistRef|nil  选中或所在歌单
---@field now_playing mineral.Song|nil  当前播放歌曲
---@field selected_loved boolean|nil  选中歌曲的收藏状态
---@field search_query string|nil  搜索或过滤词，空词为 nil
---@field args string[]  CLI 位置参数；按键触发时为空数组

--- 注册具名动作(物理键解耦,多 client 共用触发面)。重名 / 空名报错。
--- 触发面:TUI `tui.keys.script` 绑键(ctx 带按键上下文)/ CLI `mineral action <name>`(ctx 空表)。
---@param name string  动作注册名,如 "my.skip_short"
---@param handler fun(ctx: mineral.ActionCtx): nil
function mineral.action(name, handler) end

--- 语法糖:匿名动作 + 键位一步绑定(= `mineral.action(内部名, handler)` +
--- 键合进 TUI keymap)。键字符串文法与 `tui.keys` 一致(nvim 表示法,如 "X" / "<C-g>");
--- 非法键名在 TUI 侧 warn 跳过,不影响其余绑定。
---@param key string  键字符串,如 "X" / "<C-g>"
---@param handler fun(ctx: mineral.ActionCtx): nil
function mineral.bind(key, handler) end

--- 可观测属性名(字符串枚举;与 Rust `PropKey` 由守卫测试钉死同步)。
---@alias mineral.PropName "player.song"|"player.state"|"player.volume"|"player.position"|"player.mode"|"queue.length"|"terminal"

--- 终端状态，无客户端时为 nil
---@class mineral.TerminalState
---@field rows integer  终端行数
---@field cols integer  终端列数
---@field fullscreen boolean  是否全屏播放
---@field focused boolean  窗口是否聚焦；不支持焦点事件时为 true

--- 播放模式
---@alias mineral.PlayMode "sequential"|"shuffle"|"repeat_all"|"repeat_one"

--- 播放态。
---@alias mineral.PlayerState "playing"|"paused"|"stopped"

--- 订阅属性树变更(订阅即回放当前值;高频变化合并只回末值)。
--- 回调收裸值;按属性名字面量分派出对应的值类型。
---@param prop mineral.PropName
---@param on_change fun(value: any): nil
---@overload fun(prop: "player.song", on_change: fun(value: string|nil))
---@overload fun(prop: "player.state", on_change: fun(value: mineral.PlayerState))
---@overload fun(prop: "player.volume", on_change: fun(value: integer))
---@overload fun(prop: "player.position", on_change: fun(value: integer))
---@overload fun(prop: "player.mode", on_change: fun(value: mineral.PlayMode))
---@overload fun(prop: "queue.length", on_change: fun(value: integer))
---@overload fun(prop: "terminal", on_change: fun(value: mineral.TerminalState|nil))
function mineral.observe(prop, on_change) end

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
--- 成功 `on_value(值, nil)`(未命中值为 nil);失败 `on_value(nil, 错误串)`。
---@param song_id string  歌曲 id(`namespace:value` 全限定形式)
---@param key string  开放键(建议带 `.` 前缀,如 "plugin.skipcount")
---@param on_value fun(value: mineral.StoreValue, err: string|nil): nil
function mineral.store.get(song_id, key, on_value) end

--- 写 per-song 持久值(fire-and-forget;`nil` 删除该 key)。
--- 保留键(`local_play_count` / `rating` / `last_played`)拒写。
---@param song_id string
---@param key string
---@param value mineral.StoreValue
function mineral.store.set(song_id, key, value) end

--- per-song 数值自增(key 不存在以 delta 起步;现有值非整数报错)。
--- 带回调时 `on_value(自增后的值, nil)` / `on_value(nil, 错误串)`。
---@param song_id string
---@param key string
---@param delta integer  增量(可负)
---@param on_value? fun(value: integer|nil, err: string|nil): nil
function mineral.store.inc(song_id, key, delta, on_value) end

---@class mineral.queue
mineral.queue = {}

--- 读当前播放队列(回调风格;数组顺序即队列顺序)。跳播用 `mineral.player.play(song.id)`。
---@param on_songs fun(songs: mineral.Song[], err: string|nil): nil
function mineral.queue.list(on_songs) end

--- 整表重排队列:传入的数组顺序即新的队列顺序。
---
--- 只读每项的 `id`,实体由 daemon 从当前队列回捞——因此**只能删减与排序**,
--- 每个 id 必须在当前队列里出现过(次数不限,复制已在队列的歌是允许的);
--- 混入外来 id 则整次重排被拒、队列不动。加新歌用 `mineral.player.play`
--- 之类的入队路径。
---@param songs mineral.Song[]  新的队列顺序
function mineral.queue.set(songs) end

--- 队列位置，下标从 1 开始
---@class mineral.QueueCtx
---@field current integer  在播条目的下标
---@field selected integer|nil  光标下标，无光标时为 nil

--- 歌单摘要，曲目另用 library.tracks 获取
---@class mineral.PlaylistBrief
---@field id string  全局 ID（namespace:value）
---@field name string  歌单名
---@field track_count integer  曲目数
---@field description string  简介，缺失为空串
---@field play_count integer|nil  播放量
---@field subscriber_count integer|nil  收藏或订阅数
---@field source string  来源名

---@class mineral.library
mineral.library = {}

--- 读用户歌单列表(daemon 聚合快照,与 UI 所见严格一致——已过
--- `curate_playlists` 出口变换;某源拉取失败该源空贡献,不整体失败)。
--- daemon 启动早期(初始拉取未齐)回调会等到全部源就绪才触发,不会挂死。
---@param on_playlists fun(playlists: mineral.PlaylistBrief[], err: string|nil): nil
function mineral.library.playlists(on_playlists) end

--- 读指定歌单的曲目。
---@param playlist_id string  歌单 id(`namespace:value`)
---@param on_songs fun(songs: mineral.Song[], err: string|nil): nil
function mineral.library.tracks(playlist_id, on_songs) end

--- 按关键词搜索歌曲(异步回调)。
--- `opts.source` 省略 = 跨全部源聚合(单源失败跳过该源);
--- 指定则只搜该源,无对应源时回调收 `(nil, err)`。
---@param query string  关键词
---@param opts? { source?: string, offset?: integer, limit?: integer }  搜索选项(offset 默认 0,limit 默认 30)
---@param on_songs fun(songs: mineral.Song[]|nil, err: string|nil): nil
---@overload fun(query: string, on_songs: fun(songs: mineral.Song[]|nil, err: string|nil): nil): nil
function mineral.library.search(query, opts, on_songs) end

--- 可播资源，可直接用于拦截改写
---@class mineral.PlayUrl
---@field song_id string  全局歌曲 ID
---@field url string  可播地址
---@field quality string  请求音质，不代表实际音质
---@field bitrate_bps integer|nil  实际码率
---@field size integer|nil  文件大小（字节）
---@field format string|nil  容器格式
---@field headers string[][]  请求头，{{name, value}} 数组
---@field layout "contiguous"|"chunked"  流容器布局

--- 解析一首歌的可播 URL(异步回调):按 id 的 namespace 走对应源取流。
--- 无可播资源 / 无对应源时回调收 `(nil, err)`。
---@param song_id string  全限定歌曲 id(如 "bilibili:BV1xx:1")
---@param on_url fun(play_url: mineral.PlayUrl|nil, err: string|nil): nil
function mineral.library.song_url(song_id, on_url) end

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
---@param callback fun(): nil
---@return mineral.Timer
function mineral.timer.after(ms, callback) end

--- 周期定时器:每 `ms` 毫秒触发(慢回调不会重入 —— 脚本线程串行)。
---@param ms integer
---@param callback fun(): nil
---@return mineral.Timer
function mineral.timer.every(ms, callback) end

---@class mineral.ui
mineral.ui = {}

---@class mineral.config
mineral.config = {}

--- 带样式的行内文本，未指定样式时沿用上下文
---@class mineral.Span
---@field [1] string  文本内容
---@field fg? "text"|"subtext"|"overlay"|"accent"|"red"|"yellow"|"green"|"peach"|string 前景色，主题色名或 #rrggbb
---@field bold? boolean 粗体
---@field italic? boolean 斜体
---@field underline? boolean 下划线
---@field dim? boolean 暗淡
---@field align? "left"|"center"|"right" 行内分组对齐，仅整行内容生效

--- 推送单行 toast 到 client(同 id 替换不堆叠;多行内容截首行)。
--- msg 是 `print` 式宽容:任意值经 tostring 显示;**nil 静默跳过**
--- (`toast(ctx.search_query)` 这类可空链无词时安静,不报错);
--- 传 span 数组得行内样式,如 `{ "音量 ", { "42", fg = "accent", bold = true } }`。
---@param msg any|(string|mineral.Span)[]  显示内容(nil 跳过;表按 span 数组解析;其余经 tostring)
---@param opts? { kind?: "info"|"warn"|"error", id?: string, ttl_secs?: integer }  ttl_secs 缺省用 client 配置(toast.flash_ttl_secs)
function mineral.ui.toast(msg, opts) end

--- 推送多行通知卡片到 client(同 id 替换不堆叠)。
--- title 是字符串或 span 数组(画进边框);body 每项是一行:字符串(整行默认样式,
--- 内嵌 `\n` 拆成多行)或 span 数组(行内混排样式)。
--- `ttl_secs` 给了到时自动退场,边框暗色随剩余时间自左上向右下蔓延(倒计时指示);
--- 缺省驻留,用户按关闭键才消失。
---@param opts { title?: string|(string|mineral.Span)[], kind?: "info"|"warn"|"error", id?: string, ttl_secs?: integer, body: (string|(string|mineral.Span)[])[] }
function mineral.ui.card(opts) end

--- 窗口标题整串覆盖(脚本自渲染;daemon 重启即清)。
--- 渲染产物直通:不进配置合成,高频刷(轮换 / spinner)零成本;
--- `text = nil` 撤销,client 回落结构化模板(配置 tui.window_title)。
---@param text string|nil  覆盖文本;nil = 撤销
function mineral.ui.window_title(text) end

--- session 级配置覆盖(daemon 重启即清,不写配置文件)。
--- 推荐传一张配置偏表(结构同 config.lua 返回表,只写要覆盖的字段,全程补全 +
--- 类型检查):daemon 拍平成叶子深合并进有效配置并落型校验,坏路径 / 坏值被剔除
--- 并警告,结果推给所有订阅 client。
--- 字符串形 `override(path, value)` 保留:path 须是**真实配置路径**(如
--- "tui.lyrics.fullscreen_line_gap"),动态拼 path 时仍用它;`value = nil` 撤销
--- 覆盖,回落配置文件的值(表形写不出 nil,撤销只有字符串形)。
---@param patch mineral.Config  配置偏表(只写要覆盖的字段)
---@overload fun(path: string, value: mineral.BusPayload|nil)
function mineral.config.override(patch) end

--- Mineral 版本
---@class mineral.SysVersion
---@field major integer 主版本
---@field minor integer 次版本
---@field patch integer 修订版本
local SysVersion = {}

--- 拼回 `"x.y.z"` 字符串形(日志 / toast 拼串用)。
---@return string
function SysVersion:str() end

--- daemon 所在机器的路径，解析失败为 nil
---@class mineral.SysPaths
---@field config string|nil  配置目录(~/.config/mineral)
---@field data string|nil  数据目录(~/.local/share/mineral)
---@field cache string|nil  缓存目录(~/.cache/mineral)
---@field log string|nil  日志文件(<cache>/mineral.log)
---@field socket string|nil  daemon IPC socket 路径

--- 系统信息，加载后不变
---@class mineral.sys
---@field name "Mineral"  应用名称
---@field os "linux"|"macos"  编译目标操作系统
---@field arch string  CPU 架构
---@field hostname string|nil  主机名
---@field version mineral.SysVersion  mineral 版本
---@field paths mineral.SysPaths  关键路径
mineral.sys = {}

---@class mineral.log
mineral.log = {}

---@param msg string
function mineral.log.info(msg) end

---@param msg string
function mineral.log.warn(msg) end

return mineral
