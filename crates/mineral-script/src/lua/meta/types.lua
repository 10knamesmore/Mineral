---@meta _
-- daemon/tui 模块与配置回调共用的数据类型;不声明全局 host。

--- 歌曲;队列和搜索返回此形态,不带歌单位置。
---@class mineral.Song
---@field id string 全局 ID(namespace:value)
---@field title string 歌名
---@field duration_ms integer|nil 时长(毫秒),未知为 nil
---@field artists string[] 艺人名,主艺人在前
---@field album string|nil 专辑名
---@field cover_url string|nil 封面 URL 或本地路径
---@field source_url string|nil 原始资源路径或 URL
---@field source string 来源名
---@field url string|nil 网页分享链接

--- 歌单条目;数组顺序与来源位置是两件事。
---@class mineral.PlaylistEntry
---@field index integer 来源歌单中的 0-based 位置,可不连续
---@field song mineral.Song 歌曲实体

--- 复制模板的歌单形态。
---@class mineral.Playlist
---@field id string 全局 ID(namespace:value)
---@field name string 歌单名
---@field description string 简介,缺失为空串
---@field track_count integer 总曲目数,可能多于已加载曲目
---@field cover_url string|nil 封面 URL
---@field source string 来源名
---@field url string|nil 网页分享链接
---@field songs mineral.Song[] 已加载曲目

--- 复制模板的专辑形态。
---@class mineral.Album
---@field id string 全局 ID(namespace:value)
---@field name string 专辑名
---@field artists string[] 艺人名,主艺人在前
---@field description string 简介,缺失为空串
---@field track_count integer|nil 总曲目数
---@field cover_url string|nil 封面 URL
---@field source string 来源名
---@field url string|nil 网页分享链接
---@field songs mineral.Song[] 已加载曲目

--- 复制模板的艺人形态。
---@class mineral.Artist
---@field id string 全局 ID(namespace:value)
---@field name string 艺名
---@field description string 简介,缺失为空串
---@field follower_count integer|nil 关注者数
---@field album_count integer|nil 专辑数
---@field song_count integer|nil 歌曲数
---@field avatar_url string|nil 头像 URL
---@field source string 来源名
---@field url string|nil 网页分享链接
---@field songs mineral.Song[] 代表曲

--- 歌单摘要;曲目由 daemon 的 library.tracks 获取。
---@class mineral.PlaylistBrief
---@field id string 全局 ID(namespace:value)
---@field name string 歌单名
---@field track_count integer 曲目数
---@field description string 简介,缺失为空串
---@field play_count integer|nil 播放量
---@field subscriber_count integer|nil 收藏或订阅数
---@field source string 来源名

--- 队列变换的下标从 1 开始,不同于 PlaylistEntry.index。
---@class mineral.QueueCtx
---@field current integer 在播条目下标
---@field selected integer|nil 光标下标

---@alias mineral.PlayMode "sequential"|"shuffle"|"repeat_all"|"repeat_one"
---@alias mineral.StoreValue integer|number|string|boolean|nil

--- 配置覆盖可携带的 Lua 值;数组和字符串键映射不能在同一层混用。
---@alias mineral.ConfigValue nil|boolean|number|string|table

--- 可播资源,可直接作为 hook 改写结果。
---@class mineral.PlayUrl
---@field song_id string 全局歌曲 ID
---@field url string 可播地址
---@field quality string 请求音质,不代表实际音质
---@field bitrate_bps integer|nil 实际码率
---@field size integer|nil 字节数
---@field format string|nil 容器格式
---@field headers string[][] {{name, value}} 请求头数组
---@field layout "contiguous"|"chunked" 流容器布局

---@alias mineral.HookName "before_stream"|"before_download"

--- 音乐拦截上下文;resolve 配合 daemon 模块的 DEFER 使用。
---@class mineral.HookCtx
---@field song mineral.Song 触发拦截的歌
---@field kind mineral.HookName 拦截点
---@field resolve fun(decision: nil|boolean|mineral.HookReturn): nil 只认第一次;超时后丢弃

---@class mineral.BeforeStreamCtx: mineral.HookCtx
---@field mode "immediate"|"prefetch" 即时起播或预取
---@field url string|nil 原播放 URL,解析失败为 nil
---@field quality string 请求音质
---@field unplayable boolean 是否无可播 URL

---@class mineral.BeforeDownloadCtx: mineral.HookCtx
---@field url string|nil 原下载直链,解析失败为 nil
---@field quality string 请求音质
---@field unplayable boolean 是否无下载直链

--- 只填要改写的字段;skip 优先于其他字段。
---@class mineral.HookReturn
---@field url? string 替代 URL
---@field quality? string 替代音质
---@field headers? string[][] {{name, value}} 请求头数组
---@field layout? "contiguous"|"chunked" 改 URL 时省略则为 chunked
---@field bitrate_bps? integer 实际码率,供显示
---@field format? string 容器格式
---@field skip? string 跳过原因

--- 带样式的行内文本;未指定样式时沿用上下文。
---@class mineral.Span
---@field [1] string 文本
---@field fg? "text"|"subtext"|"overlay"|"accent"|"red"|"yellow"|"green"|"peach"|string 主题色名或 #rrggbb
---@field bold? boolean
---@field italic? boolean
---@field underline? boolean
---@field dim? boolean
---@field align? "left"|"center"|"right" 行内分组对齐,仅整行生效

--- 当前执行进程的版本。
---@class mineral.SysVersion
---@field major integer 主版本
---@field minor integer 次版本
---@field patch integer 修订版本
local SysVersion = {}

---@return string version x.y.z
function SysVersion:str() end

--- 当前执行进程所在机器的路径;解析失败为 nil。
---@class mineral.SysPaths
---@field config string|nil 配置目录
---@field data string|nil 数据目录
---@field cache string|nil 缓存目录
---@field log string|nil 日志文件
---@field socket string|nil IPC socket

---@class mineral.Sys
---@field name "Mineral"
---@field os "linux"|"macos"
---@field arch string CPU 架构
---@field hostname string|nil 主机名
---@field version mineral.SysVersion
---@field paths mineral.SysPaths

---@class mineral.Log
local Log = {}

--- 写入当前进程日志。
---@param msg string
function Log.info(msg) end

--- 写入当前进程警告日志。
---@param msg string
function Log.warn(msg) end

---来源名，未加载的来源跳过
---@alias mineral.SourceName "netease"|"bilibili"|"local"|"mineral"|string

---搜索目标类型
---@alias mineral.SearchKind "song"|"album"|"artist"|"playlist"|"user"

---音质档位，由低到高
---@alias mineral.BitRate "standard"|"higher"|"exhigh"|"lossless"|"hires"
