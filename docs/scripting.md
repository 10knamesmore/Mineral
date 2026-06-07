# Mineral Lua 脚本指南

`config.lua` 不只是配置文件——它跑在 daemon 内嵌的 Lua 5.4 VM 里,顶层任何 `mineral.*` 调用都是真实生效的脚本。本文是人读的完整指南;编辑器内的权威 API 签名见 `mineral config init` 生成的类型注解(`meta/mineral.lua`),装了 [lua-language-server](https://github.com/LuaLS/lua-language-server) 即有补全与类型检查。

## 心智模型

- **脚本在 daemon 里跑**,不在 TUI 里。TUI 只是触发面之一(绑键)和输出面之一(toast)。CLI、未来的其他 client 共享同一份脚本。
- **单线程串行**:所有回调在一条专用脚本线程上排队执行,无重入、无并发,不需要锁。代价是慢回调会排队拖后面的——耗时活交给 `mineral.spawn`(子进程)或拆小。
- **错误被隔离**:单个回调出错只记日志 + toast 提示,其他回调、播放本体不受影响;脚本整体加载失败时 daemon 照常启动(无脚本模式)。
- **看门狗**:回调超 100ms 记 warn,超 1000ms 被强制中断(只杀本次调用,脚本仍存活)。阈值可调,见文末配置表。
- **热重载**:保存 `config.lua`,daemon 自动重建脚本 VM——注册全部重来,`observe` 立即回放当前值,定时器清零;新脚本有语法错误时**保留旧脚本继续跑**,错误经 toast 报给你。

## Hello World

`config.lua` 整个文件是一个 Lua chunk:**脚本写在顶层、`return` 之前**(Lua 的 `return` 必须是最后一条语句);`return` 的表是纯配置数据,里面不放 `mineral.*` 调用。

```lua
-- ~/.config/mineral/config.lua

-- ① 脚本:顶层 mineral.* 调用,文件被 eval 时立即执行(注册回调 / 绑键)
mineral.bind("X", function(ctx)
    mineral.ui.toast("你按了 X,当前视图:" .. (ctx.view or "?"))
end)

-- ② 配置:最后 return 配置表(只写想改的;全默认就 return {})
return {
    tui = {
        behavior = { volume_step = 10 },
    },
}
```

保存后(daemon 在跑则热重载,否则下次启动生效),TUI 里按 `X` 即见 toast。

## 通用约定

| 约定           | 说明                                                                                          |
| -------------- | --------------------------------------------------------------------------------------------- |
| 歌曲 / 歌单 id | 全限定字符串 `"namespace:value"`(如 `"netease:123"`),回调给出的 id 可直接回喂任何 API         |
| 异步回调风格   | 查询类 API 不阻塞脚本线程,结果回调 `fn(value, err)`:成功 `err` 为 `nil`,失败 `value` 为 `nil` |
| 音质名         | `"standard" \| "higher" \| "exhigh" \| "lossless" \| "hires"`                                 |

---

## 触发面:什么时候跑你的代码

### 事件 `mineral.on(event, fn)`

离散生命周期事件,回调收单一 args table:

| 事件                   | args                                                                                  | 时机                                                                                 |
| ---------------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `"track_started"`      | `{ song }`                                                                            | 在播曲目变更(远端起播 / 本地命中 / gapless 推进全覆盖;同曲重启 / 单曲循环不重复触发) |
| `"track_finished"`     | `{ song, reason }`,reason ∈ `eof / skip / error / stop`                               | 一首歌结束(自然播完 / 切歌 / 出错 / 停止)                                            |
| `"download_completed"` | `{ song, path, quality, format }`(quality 为有效音质名;format 如 `"flac"`,拿不到 nil) | 一首歌下载落盘完成(已存在跳过不触发)                                                 |

```lua
mineral.on("track_finished", function(args)
    if args.reason == "skip" then
        mineral.store.inc(args.song.id, "plugin.skips", 1)
    end
end)
```

### 属性 `mineral.observe(prop, fn)` / `mineral.get(prop)`

持续状态的订阅与读取。**订阅即回放**(注册时有当前值立刻调一次);高频变化合并只回末值。

| 属性                | 回调值类型                                                  | 说明                                  |
| ------------------- | ----------------------------------------------------------- | ------------------------------------- |
| `"player.song"`     | `string \| nil`                                             | 在播歌的 id,无在播为 nil              |
| `"player.state"`    | `"playing" \| "paused" \| "stopped"`                        | 播放态                                |
| `"player.volume"`   | integer                                                     | 音量 0-100                            |
| `"player.position"` | integer                                                     | 播放进度(整秒)                        |
| `"player.mode"`     | `"sequential" \| "shuffle" \| "repeat_all" \| "repeat_one"` | 循环模式                              |
| `"queue.length"`    | integer                                                     | 队列长度                              |
| `"terminal"`        | `{ rows, cols, fullscreen } \| nil`                         | 终端尺寸与全屏态;无 client 在线为 nil |

`mineral.get(prop)` 同步读当前值(还没推送过为 `nil`)。

### 动作 `mineral.action(name, fn)` 与 `mineral.bind(key, fn)`

具名动作把「物理键」与「行为」解耦:

```lua
mineral.action("my.love_and_next", function(ctx)
    if ctx.now_playing then
        mineral.library.love(ctx.now_playing.id, true)
        mineral.player.next()
    end
end)
```

触发面有两个:

- **TUI 绑键**:配置 `tui.keys.script = { ["my.love_and_next"] = "L" }`
- **CLI**:`mineral action my.love_and_next`(脚本调试利器)

`mineral.bind(key, fn)` 是语法糖 = 匿名 action + 键位一步绑定。

回调的 `ctx` 携带按键瞬间的 client 上下文(CLI 触发时全为 `nil`):

| 字段                | 类型                                                             | 说明             |
| ------------------- | ---------------------------------------------------------------- | ---------------- |
| `view`              | `"playlists" \| "tracks" \| "queue" \| "fullscreen" \| "search"` | 按键时所在视图   |
| `selected_song`     | Song                                                             | 列表光标选中的歌 |
| `selected_playlist` | `{ id, name }`                                                   | 选中 / 所在歌单  |
| `now_playing`       | Song                                                             | 在播的歌         |
| `selected_loved`    | boolean                                                          | 选中歌的 ♥ 态    |
| `search_query`      | string                                                           | 当前搜索词       |

播放器自身的状态(音量 / 进度 / 模式)不在 ctx 里——用 `mineral.get`。

### 定时器 `mineral.timer`

```lua
local t = mineral.timer.every(60 * 1000, function()
    mineral.log.info("还活着")
end)
-- t:stop()    暂停(冻结剩余计时)
-- t:resume()  续跑
-- t:kill()    注销(幂等;after 触发后自动注销)

mineral.timer.after(5000, function() mineral.ui.toast("5 秒到") end)
```

慢回调不会重入(脚本线程串行)。热重载后定时器清零,需要常驻的写在脚本顶层让重载重建。

### 同步拦截 `mineral.hook(name, fn)`

播放 / 下载链路上的**同步**裁决点——daemon 拿到 URL 后、真正使用前,等你的返回值:

| 拦截点              | 时机                          |
| ------------------- | ----------------------------- |
| `"before_play"`     | 远端解析出播放 URL 后、起播前 |
| `"before_download"` | 取到下载直链后、写盘前        |

回调收 `ctx = { song, url, quality, kind }`,返回值契约:

| 返回                               | 效果                                            |
| ---------------------------------- | ----------------------------------------------- |
| `nil` 或 `true`                    | 放行,原样继续                                   |
| `false` 或 `{ skip = "原因" }`     | 跳过本次(播放跳下一首并 toast 原因;下载记 skip) |
| `{ url = "...", quality = "..." }` | 改写后继续(字段都可选,只给要改的)               |

```lua
-- 拒播 30 秒以下的短音频(广告/试听残片)
mineral.hook("before_play", function(ctx)
    if ctx.song.duration_ms > 0 and ctx.song.duration_ms < 30 * 1000 then
        return { skip = "短于 30s" }
    end
end)
```

要点:

- **回调要快**:超过 `script.hook_timeout_ms`(默认 2000ms)按放行处理,不会卡死播放
- 同一拦截点可注册多个,按注册顺序调用,**首个非放行返回短路生效**
- 改写过的播放流**不进缓存**(缓存按原曲入键,改写内容自负)
- 本地缓存命中与 gapless 预排路径不过 hook(前者是你自己的文件,后者 decoder 已就绪、物理上没有拦截窗口)——hook 是**拦截点**不是观察点,「每次开始播放做点什么」请用 `on("track_started")`,它全路径覆盖

### 自定义总线 `mineral.emit` / `mineral.on_message`

脚本内部、以及脚本与外部工具之间的自由消息通道,daemon 零解释转发:

```lua
mineral.on_message("my.refresh", function(payload)
    mineral.ui.toast("收到:" .. tostring(payload and payload.n))
end)

mineral.emit("my.refresh", { n = 42 })   -- 本 VM 订阅者同步收到
```

载荷规则:标量与嵌套 table(数组形或字符串键映射),嵌套 ≤ 8 层,同一层不混用数组键与字符串键,不支持 function / userdata。

---

## 数据与控制

### 播放控制 `mineral.player`

```lua
mineral.player.toggle()          -- 播放 / 暂停
mineral.player.next()            -- 下一首
mineral.player.prev()            -- 上一首 / 回开头
mineral.player.stop()
mineral.player.seek_rel(-10)     -- 相对 seek(秒,可负)
mineral.player.seek_to(60)       -- 绝对 seek
mineral.player.set_volume(80)    -- 越界 clamp,不报错
mineral.player.set_mode("shuffle")
mineral.player.play("netease:123")
```

### 队列与曲库

```lua
mineral.queue.list(function(songs, err) ... end)           -- 当前队列(只读)
mineral.library.playlists(function(playlists, err) ... end) -- 用户歌单(跨源聚合)
mineral.library.tracks("netease:pid", function(songs, err) ... end)
mineral.library.search("关键词", { source = "netease", limit = 10 },
    function(songs, err) ... end)                           -- opts 可省略
mineral.library.love("netease:123", true)                   -- 设 ♥(本地 + 远端)
mineral.download("netease:123")                              -- 下载导出
```

### per-song 持久 KV `mineral.store`

跟着每首歌走的持久化存储(daemon 重启仍在),开放给脚本自由使用:

```lua
mineral.store.set("netease:123", "plugin.note", "现场版更好听")
mineral.store.get("netease:123", "plugin.note", function(v, err) ... end)
mineral.store.inc("netease:123", "plugin.full_plays", 1, function(v) ... end)
mineral.store.set("netease:123", "plugin.note", nil)   -- nil = 删除
```

- 值是标量:integer / number / string / boolean
- 键建议带 `.` 前缀命名空间(如 `plugin.xxx`),与未来一等字段隔开
- 保留键拒写:`local_play_count` / `rating` / `last_played`

---

## 对外输出

### 提示 `mineral.ui.toast(msg, opts?)`

```lua
mineral.ui.toast("hello")                                    -- 一次性堆叠
mineral.ui.toast("音量 80", { id = "vol" })                  -- 同 id 顶替不堆叠
mineral.ui.toast("出事了", { kind = "error", ttl_secs = 10 })
```

`msg` 是 `print` 式宽容:任意值经 tostring;`nil` 静默跳过。

### UI 旋钮覆盖 `mineral.ui.override(key, value)`

session 级覆盖 TUI 渲染旋钮(不写配置文件,daemon 重启即清;`value = nil` 撤销回落配置值):

| key                            | 类型        | 旋钮             |
| ------------------------------ | ----------- | ---------------- |
| `"lyrics.fullscreen_line_gap"` | integer ≥ 0 | 全屏歌词行间距   |
| `"lyrics.compact_line_gap"`    | integer ≥ 0 | 非全屏歌词行间距 |

```lua
-- 终端宽度自适应:超 200 列拉开全屏歌词行距
mineral.observe("terminal", function(t)
    mineral.ui.override("lyrics.fullscreen_line_gap",
        (t and t.cols > 200) and 2 or nil)
end)
```

未知 key 由 client 警告并忽略,不报错。

### 子进程 `mineral.spawn(args, opts?, fn)`

把耗时活、系统联动甩给外部进程,异步回调结果:

```lua
local handle = mineral.spawn(
    { "notify-send", "Mineral", "下载完成" },
    function(result, err)
        if err then mineral.log.warn("spawn 失败:" .. err) end
    end)
-- handle:kill()   中止(SIGKILL)
```

- `args` 是字符串数组(首元素为可执行文件),**不经 shell**——不用担心引号注入
- `opts` 可带 `{ cwd = "...", env = { K = "V" } }`
- 回调收 `result = { code, stdout, stderr, killed }`(被信号杀时 `code` 为 nil)
- 并发上限 `script.spawn_max_concurrent`(默认 8),超限回调收 `(nil, err)`

### 日志 `mineral.log`

`mineral.log.info(msg)` / `mineral.log.warn(msg)` 写进 daemon 日志(`~/.cache/mineral/mineral.log`),排错主通道。

### 系统信息 `mineral.sys`

host 独有的常量信息(加载时灌入,运行期不变):

```lua
mineral.sys.os         -- "linux" | "macos"
mineral.sys.arch       -- "x86_64" / "aarch64"
mineral.sys.hostname   -- 主机名(双机共享一份 config.lua 时分叉用)
mineral.sys.version    -- { major = 0, minor = 5, patch = 0 }(结构化,共享配置做兼容分叉)

mineral.sys.paths.config   -- ~/.config/mineral
mineral.sys.paths.data     -- ~/.local/share/mineral(脚本自己的持久文件放这)
mineral.sys.paths.cache    -- ~/.cache/mineral
mineral.sys.paths.log      -- ~/.cache/mineral/mineral.log
mineral.sys.paths.socket   -- daemon IPC socket
```

配置写大了可以按文件拆分:

```lua
dofile(mineral.sys.paths.config .. "/lua/my_plugin.lua")
```

两个有意的「没有」:时间日期**用 Lua 标准库**(`os.time()` 实时时间戳、`os.date("*t")` 实时结构化表,不做重复 API);**不提供 cwd**——脚本跑在 daemon 里,daemon 的 cwd 取决于谁拉起它(终端 / systemd),无稳定语义,文件操作用 `paths.*`、子进程工作目录用 `spawn` 的 `opts.cwd`。

---

## Recipes

### 睡眠定时器(按一下设定,再按取消)

```lua
local sleep
mineral.bind("S", function()
    if sleep then
        sleep:kill(); sleep = nil
        mineral.ui.toast("睡眠定时器已取消", { id = "sleep" })
    else
        sleep = mineral.timer.after(30 * 60 * 1000, function()
            mineral.player.stop(); sleep = nil
        end)
        mineral.ui.toast("30 分钟后停止播放", { id = "sleep" })
    end
end)
```

### 烂歌自动跳(行为驱动的个人黑名单)

手动跳过 ≥3 次的歌,以后轮到它直接跳——不用手动维护任何列表。
计数持久在 per-song KV(跨重启);hook 是同步的、不能等异步查询,
所以用 Lua 表做内存缓存,跳过发生时顺手更新:

```lua
local skips = {}

mineral.on("track_finished", function(args)
    if args.reason ~= "skip" then return end
    mineral.store.inc(args.song.id, "plugin.skips", 1, function(n)
        skips[args.song.id] = n
    end)
end)

mineral.hook("before_play", function(ctx)
    if (skips[ctx.song.id] or 0) >= 3 then
        return { skip = "跳过 3 次,自动拉黑" }
    end
end)
```

局限:热重载/重启后内存缓存清空,需要这首歌再被跳一次才会重新拉黑
(持久计数还在,不会从头数)。想解除拉黑:`mineral.store.set(id, "plugin.skips", nil)`。

### ListenBrainz scrobble(完播上报)

内置没有任何远端 scrobble;`spawn` + curl 十几行搞定:

```lua
local TOKEN = "你的 ListenBrainz token"

mineral.on("track_finished", function(args)
    if args.reason ~= "eof" then return end
    local payload = string.format(
        '{"listen_type":"single","payload":[{"listened_at":%d,'
            .. '"track_metadata":{"track_name":%q}}]}',
        os.time(), args.song.title)
    mineral.spawn({
        "curl", "-s", "-X", "POST",
        "https://api.listenbrainz.org/1/submit-listens",
        "-H", "Authorization: Token " .. TOKEN,
        "-H", "Content-Type: application/json",
        "-d", payload,
    }, function(r, err)
        if err or (r and r.code ~= 0) then
            mineral.log.warn("scrobble 失败:" .. (err or r.stderr))
        end
    end)
end)
```

### 切歌桌面通知(跨平台)

```lua
-- 注意别写 `local os = ...`:会遮蔽 Lua 标准库 os(os.date / os.time)
local sys_os = mineral.sys.os
local app_name = mineral.sys.name
local version = mineral.sys.version

mineral.on("track_started", function(args)
  -- album 拿不到时是 nil,拼接前要兜底
  local body = args.song.title .. " - " .. (args.song.album or "未知专辑")
  local cmd
  if sys_os == "macos" then
    cmd = {
      "osascript",
      "-e",
      ('display notification %q with title %q'):format(body, app_name),
    }
  else
    cmd = {
      "notify-send",
      "-a",
      app_name .. " " .. version:str(),
      "♪ 正在播放",
      body,
    }
  end
  mineral.spawn(cmd, function() end)
end)
```

`track_started` 与 `player.song` 属性同源:远端起播、本地缓存命中、
gapless 自动推进全覆盖。同曲重启(`p` 回开头、单曲循环)不重复触发——
对通知场景这正是想要的行为。

### 下载自动同步到 NAS(按歌手 / 专辑归档)

`download_completed` 的 args 带齐了归档要素:`song.artists` / `song.album` /
`quality` / `format`,远端目录结构随你组织:

```lua
mineral.on("download_completed", function(args)
    local s = args.song
    local dest = ("nas:/music/%s/%s/"):format(
        s.artists[1] or "未知歌手",
        s.album or "未知专辑")
    mineral.spawn({ "rsync", "-a", args.path, dest }, function(r, err)
        if err or (r and r.code ~= 0) then
            mineral.ui.toast(("同步 NAS 失败:%s(%s)"):format(s.title, args.quality),
                { kind = "warn" })
        end
    end)
end)
```

### 宽终端自适应歌词行距

终端尺寸是脚本可观察的属性,UI 旋钮可以被 session 级覆盖:

```lua
mineral.observe("terminal", function(terminal)
  if terminal == nil then
    return
  end

  if terminal.cols > 200 then
    mineral.ui.override("lyrics.fullscreen_line_gap", 2)
    mineral.ui.override("lyrics.compact_line_gap", 1)
  else
    mineral.ui.override("lyrics.fullscreen_line_gap", nil)
    mineral.ui.override("lyrics.compact_line_gap", nil)
  end
end)
```

### 深夜自动降音量

```lua
mineral.timer.every(10 * 60 * 1000, function()
    local hour = tonumber(os.date("%H"))
    if hour >= 23 or hour < 7 then
        local v = mineral.get("player.volume")
        if v and v > 40 then mineral.player.set_volume(40) end
    end
end)
```

### 搜索词直接全网播(列表里没有就去搜)

TUI 的 `/` 只过滤本地已加载列表;绑个键把当前搜索词直接搜全源、播第一条:

```lua
mineral.bind("P", function(ctx)
    if not ctx.search_query then return end
    mineral.library.search(ctx.search_query, { limit = 1 },
        function(songs, err)
            if songs and songs[1] then
                mineral.player.play(songs[1].id)
            else
                mineral.ui.toast("没搜到:" .. ctx.search_query,
                    { kind = "warn" })
            end
        end)
end)
```

---

## 运行时配置(`config.lua` 的 `script` 段)

| 旋钮                            | 默认 | 说明                                         |
| ------------------------------- | ---- | -------------------------------------------- |
| `watchdog_instruction_interval` | 2000 | 每多少条 VM 指令查一次墙钟;小 = 灵敏但开销大 |
| `watchdog_soft_wall_ms`         | 100  | 回调超此时长记 warn,继续跑                   |
| `watchdog_hard_wall_ms`         | 1000 | 回调超此时长被中断(只杀本次调用)             |
| `hook_timeout_ms`               | 2000 | 拦截 hook 软超时;超时按放行处理              |
| `spawn_max_concurrent`          | 8    | 子进程并发上限;0 = 不限                      |

## 排错

- **日志**:`~/.cache/mineral/mineral.log`,脚本相关条目 target 是 `script`
- **手动触发**:`mineral action <名字>` 不开 TUI 直接调动作,看输出最快
- **回调被中断**:日志里有 watchdog 记录;检查是否做了同步耗时活,改 `mineral.spawn`
- **重载没生效**:语法错误时保留旧脚本,toast 会报错误位置;`mineral config check` 离线验语法
- **hook 没拦到**:本地缓存命中与 gapless 预排不过 hook,属预期
