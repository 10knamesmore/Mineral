# Changelog
## [0.5.1] — 2026-06-10

### Breaking Changes

- 歌词单轨化,翻译/罗马音装配期互最近邻配对 ([`2df781d`](https://github.com/10knamesmore/Mineral/commit/2df781d3499dfe0673e77ee259dd8f04fb5dd058))

### Features

- IPC 优雅停止 — mineral stop 命令 + TUI Shift+Q ([`c91beee`](https://github.com/10knamesmore/Mineral/commit/c91beeeb8c152178d16007d87e8d947d313b824c))

- 统一歌词模型 + 全屏沉浸手动滚动 ([`ab7ecdd`](https://github.com/10knamesmore/Mineral/commit/ab7ecddacf491d954f9a8223635fc0265747cc11))

- 沉浸滚动边界 rubber-band 回弹 ([`7ef3320`](https://github.com/10knamesmore/Mineral/commit/7ef3320dac0c65678586dd05500caf420a55391a))

- 列表 nvim 滚动手感 + scrolloff + 平滑视口滚动 ([`ba264d5`](https://github.com/10knamesmore/Mineral/commit/ba264d54f57c8f2f2e236983c5ec33fd2710e60e))

- Playlists 深度搜索 + 命中样式/定位可配 ([`1ef25f1`](https://github.com/10knamesmore/Mineral/commit/1ef25f1ae8419ab62e01a18e2ed2b6c4c7322399))

- 歌单内光标位置记忆 + 屏上相对位置精确还原 ([`a769d63`](https://github.com/10knamesmore/Mineral/commit/a769d638475fde3cad015b9ed6fe8faefc36e999))

- 多行通知卡片 + 通用样式 span + TTL 边框倒计时 ([`7584469`](https://github.com/10knamesmore/Mineral/commit/758446924a564a1e867f4f612f975a03aeeca8f0))

- 浮层/卡片进出场内容跟动(离屏窗口搬运替代纯色空壳) ([`dd7de87`](https://github.com/10knamesmore/Mineral/commit/dd7de876ec9b234431a4356c95292859a4ddd1d1))

### Bug Fixes

- Cli_smoke 在 macOS 上 socket 路径超长误报 ([`0f5651f`](https://github.com/10knamesmore/Mineral/commit/0f5651faf887bb2eda3482ab90082df41d036f3d))

- 容忍网易 t=-1 无时间轴哨兵(JSON 负 t + 畸形 [00:00.00-1]) ([`cf59006`](https://github.com/10knamesmore/Mineral/commit/cf59006833e9e7875255c6598bbbc222bc380c7e))

- Linux MPRIS 测试补齐 LyricLine 单轨化新增字段 ([`c808640`](https://github.com/10knamesmore/Mineral/commit/c80864086db32163620db8a47b64e0fbed41e2f0))
## [0.5.0] — 2026-06-07

### 功能

- Sidebar 搜索接 fuzzy 匹配 + 拼音(全拼/首字母)过滤

- 中央 Action 枚举 + Keymap 默认表(键位可配前置,PR-A 纯增)

- Lua 用户配置 sub01 — loader + schema + config CLI

- Sub02 全量声明旋钮接线 — default.lua 单一真相源

- ADSR 时间制包络 — 快攻慢放,全配置面时长旋钮 ms 化

- Transport gapless prefetch 标记 — ⏭ 旁 ⇣ 拉取中暗/就绪亮

- 跨重启恢复 play_mode + 歌词副轨档,周期落盘加空态守卫

- 协议切 Frame 管线 — request-id 配对 + Event 交错下推 + 版本守门握手

- Mineral-script crate — Lua VM 专用线程 + watchdog + 脚本 API 面

- 脚本运行时接线 — 同 VM 加载配置 + 事件双路下发 + action 触发链

- 脚本生态数据面+触发面 — store/queue/library/timer + action ctx + error reason

- 脚本生命周期 — 热重载 + bind + nvim 键语法 + Notice 退役

- 强力位四件套 — library.search + 同步拦截 hook + spawn + bus

- Client UI 通路 — terminal 复合属性 + ui.override 旋钮覆盖

- Track_started 事件 — track_finished 的对偶

- Mineral.sys 命名空间 + Song 投影丰富 + download 事件携带音质/格式

### 修复

- 频谱过渡被打断时从可见中间色继续渐变,不再跳变

### 性能

- PlayerSync 版本门控同步替换 PlayerSnapshot 全量轮询

- 缓存存加工后产物 + 64px 取色,封面管线 CPU 大降

## [0.4.2] — 2026-06-03

全屏沉浸播放态(`z` 进出)+ gapless 无缝播放 + 频谱 / 封面观感打磨。

### 功能

- **全屏播放态**:`z` 进出,封面 / 歌词 / 频谱沉浸布局,进退场整屏形变动画;全屏歌词平滑平移 + 行间距(Apple Music 风格)。
- **gapless 无缝播放**:预排下一曲 decoder + 边界轮转,曲间无空隙(此前列为「本期不做」,现已落地)。
- **频谱**:封面取色铺到频率轴、从当前可见配色缓动到静止;低频去平板 + 峰值混合增强动态;隐藏 label;全屏下加高。
- **transport**:显示在播音频规格(format · bitrate · 采样率);进度条叠加缓冲进度。
- **网易云**:读取单曲真实累计播放次数,选中歌展示。
- 全屏 queue 浮层(与浏览布局同宽高、贴右);tracks / queue 列宽随窗口响应式两档。
- 整屏 expand / collapse 以光标真实位置为缩放锚点。
- 搜索态迁入 sidebar 标题,支持 vim 退格退出(移除独立 status_bar)。

### 修复

- 全屏切歌时封面不再不加载:封面请求与浏览选中解耦,跟随在播曲并沿播放队列预取邻近封面。
- 修全屏封面残影 / 丢失。

### 性能

- 封面 resize / base64 编码离线到 worker 线程池,切歌 / 关浮层不再卡帧;全屏稳态提前编码下一首封面,自动切歌零闪。

## [0.4.1] — 2026-05-30

修缓存 / 下载库重播不显示音频格式。

### 修复

- 本地命中(缓存 / 下载库)重播时,`PlayUrl.format` 改走 lofty `Probe`(按文件内容、跳过 ID3 标签再认底层帧)。旧实现用 `FileType::from_buffer`,一见 `ID3` 前缀即整片漏判,NetEase exhigh 等 FFmpeg 转码的 mp3 格式显示为空(FLAC 因 magic 在偏移 0 不受影响)。走 `Probe::new`(reader,无路径)而非 `Probe::open`,保住「只认内容、不信扩展名」契约。下载库里带 ID3 的 mp3 同样修复。

## [0.4.0] — 2026-05-30

本地缓存 / 下载库体系成型(文件系统为真相、sqlite 索引);macOS 系统媒体集成。

### 缓存 / 下载库

- 缓存索引迁移到 **sqlite 写穿透**,弃用 BlobCache / bincode。
- 下载库改以**文件系统为真相**,移除 `download_export` 索引——历史下载 / 换机拷库 / 手动放入的文件一律可见,不受索引漂移影响。
- 下载不再复制进缓存;缓存仅由「边播边 capture」自然形成,职责分离;补端到端测「下载 → 播放走下载库」。
- 本地优先解析:播放前按音质从高到低查缓存 / 下载库,命中则跳过整条网络取链路径(同音质优先缓存,更高音质优先下载库)。
- `mineral cache status` 子命令查看缓存占用;`clean` 展示清理效果。

### 媒体集成

- macOS 系统 Now Playing 集成:Control Center + 媒体键(配合既有 MPRIS,双平台系统媒体控制就绪)。

### TUI

- 播放栏标记播放来源(cache / download / remote);本地播放显示真实 format / bitrate。
- 统一详情视图封面高度,消除 playlist / tracks 切换时的封面跳变。

### 路径 / 平台

- 统一跨平台 XDG 目录解析,加固 socket 路径解析。

### 其他

- 默认播放音质 Lossless → Exhigh(默认 `BitRate` 亦由 Higher 改 Exhigh)。

### 测试

- 真实 TCP I/O 测试改 multi_thread runtime,消除全仓并发 flaky。

## [0.2.0] — 2026-05-24

client/server 架构落地:播放进 daemon,关 TUI 不停播;接入系统媒体服务;测试覆盖成体系。

### 架构 — client / server 分离

- 抽 `mineral-server`(audio + task + 播放上下文收成 `Server` / `ClientHandle`)与 `mineral-protocol`(IPC 协议 crate,Request/Response + length-delimited + bincode)。
- `PlayerCore` 持播放上下文(队列 / 当前歌 / 歌词 / prefetch),daemon 自治 auto-next;PCM 走 wire —— 真正「关 TUI 不杀播放」。
- TUI 走 unix socket 连 daemon;默认启动 = 优先 attach 已有 daemon,否则 **spawn 独立 daemon 进程**再 attach;保留 `--connect`(强制连)/ `--in-proc`(同进程调试);`KILL_SPAWNED_DAEMON_ON_EXIT` 旋钮决定退出时是否带走自起的 daemon(待 lua 配置接管)。
- daemon graceful shutdown(收 SIGINT/SIGTERM 清 socket),信号 handler 提前到 bind 之前消除启动竞态。
- client 断连(daemon 被单独 kill)不再僵死:检测断开 → 记日志 + 盖断连提示 modal,等按键退出;TUI 进程收 SIGTERM/INT/HUP 先记日志再走正常退出,不 silent dead。

### 媒体集成(MPRIS)

- 接入系统媒体服务 `org.mpris.MediaPlayer2`:上报当前播放、响应媒体键 / 桌面控件;`xesam:asText` 同步当前歌词(给 quickshell 等)。
- Shuffle / LoopStatus 双向同步:4-variant `PlayMode` ↔(shuffle × repeat)二维无损塌缩。
- seek 时补发 `Seeked` 信号。

### 歌词

- channel 层输出结构化歌词,消费方零解析;MPRIS / UI 共用。

### 日志 / 可观测性

- 全链路结构化埋点;错误统一 `mineral_log::chain`(完整 context 链、单行、无 ANSI / backtrace)。
- 日志改人读单行格式(本地时间 + target + `file:line` + 字段),压低 symphonia / reqwest / hyper / stream-download 等第三方噪音。
- 60s 心跳(server + client 双侧)上报内部状态;netease 反序列化走 `serde_path_to_error`,错误带字段路径。

### TUI

- `top_status` 后台任务按 `ChannelFetchKind` 拆分计数,cover loading 显真实数。
- prefetch 失败的歌单不再每帧无限重提交(request-once dedup)。
- `sidebar/playlists` 列宽改 `Constraint::Fill` 消除 ratatui 列宽求解非确定(帧间列宽闪烁)。

### 测试

- 覆盖从 ~12% 提到 145+ 测试:player 队列 / shuffle / 模式逻辑、纯逻辑函数(format / layout / color)、protocol codec round-trip、netease wire 与 LRC 解析、daemon 进程级 e2e(`CARGO_BIN_EXE`)、CLI 冒烟。
- 引入 **insta 快照**:TUI 组件用 `TestBackend` 渲染 + `assert_snapshot!`(不依赖真 pty),解析层用 `assert_debug_snapshot!`;全部带中文 description,版本号用 `filters` 归一化。展示性 fixture 用真实曲目(Mineral《EndSerenading》/ Chinese Football / MyGO!!!!!《迷跡波》)。
- CLAUDE.md 新增「测试约定」节。

## [0.1.0] — 2026-05-03

首个 alpha 版本。从老仓库重写,把核心闭环跑通。

### 架构

- workspace 拆 13 个 crate,职责按 model / channel / task / audio / spectrum / tui / cli 分层。
- `MusicChannel` trait(async)统一抽象搜索 / 详情 / 播放 URL / 歌词 / 用户数据;数据模型平铺,新加 channel 不污染。
- `mineral-task`:优先级 lane(User / Background) + 取消 + dedup,封面 / 歌单 / 歌词分别走自己的 worker。
- `mineral-paths`:XDG 标准目录(config / data / cache)解析 + 跨平台 fallback。
- `mineral-log`:`tracing` 后端 + 文件 appender,业务侧用 macro facade 调。
- 全仓 `anyhow → color-eyre`;workspace 全局 lints(unsafe / unwrap / panic / as / wildcard import 一律 deny,函数 ≤ 300 行)。
- HashMap / HashSet 全部换 `FxHashMap` / `FxHashSet`(显式名,无 alias)。
- nightly toolchain + edition 2024,`rust-toolchain.toml` 钉住。

### 音频

- rodio 0.22 + symphonia + stream-download:支持 mp3 / aac / m4a / flac 流式播放。
- seek 全链路打通,`p` 键 iTunes 行为(>3s 回开头,否则上一首)。
- auto-next + 大跨度 seek(`Shift+←/→` 30s);auto-next prefetch 提前拉下一首 SongUrl,曲终命中跳过等待。
- armed 状态机过滤过期 PlayUrlReady,修切歌时误跳。
- Shuffle 一次性洗牌、Repeat / RepeatOne 循环模式。
- cubic 音量曲线,默认 100。

### 数据源

- 一个云端 channel(加密 + cookie + 端点)接入,搜索 / 歌单 / 歌曲详情 / 播放 URL / 歌词 / liked 列表全部就绪。
- mock channel(opt-in feature),离线开发不打任何端点。

### TUI

- 双视图 sidebar:playlists / library,Table 渲染,列对齐。
- now_playing 右栏:真实封面(ratatui-image,kitty / iTerm2 / sixel / halfblock 自适配),selected 面板按 cell 像素比横向铺满,字号变化按 dims 重建,滚动期间跳过 protocol 重建(80ms 防抖,参考 yazi `image_delay`)。
- 视口 prefetch:cover / playlist tracks 按 sel ± 64 提前拉。
- queue 浮层 + 全局播放键穿透(空格 / n / p / m / 音量 / seek)。
- 频谱面板:realfft 真值 + baseline + peak hold + 余韵 trail + 弹簧物理 + 色相漂移,bar 数随窗口动态。
- 歌词:LRC 行级 + YRC 字符级 wipe(30fps 字符级渐变),Apple Music 风格 fade,中心行换 accent 色。
- transport:title / artist · album / 进度条 / 播放控制 / 音量 + 循环模式 + 真实 fmt(format · bitrate)。
- 搜索过滤(`/` 触发):playlists 按 name,library 按 name / artists / album,case-insensitive,命中子串高亮(peach + bold + underline)。
- 视图切换 / Esc 清搜索词;Library 内 search 不影响选中歌单。
- 列表辅助:`g` / `G` 跳首末、`Shift+J/K` 7 行大跳、`n / m` 位置指示、`♥` gutter(loved 标记)、`♫` 当前播放标记。
- top_status:左 mineral + 真实 version + tabs,右后台 task 计数 + 播放状态。
- panic hook 链:Tui::enter 把 restore_terminal 接进 panic hook,确保彩色报告不被 alternate screen 吞。

### CLI

- `mineral channel netease login`:终端二维码扫码登录,凭证写入 `<data_dir>/netease.json`。

### 配置 / 路径

- 配置 / 数据 / 缓存目录走 XDG(`$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` / `$XDG_CACHE_HOME`,fallback `~/.config` / `~/.local/share` / `~/.cache`)。
- 日志默认写 `<cache_dir>/mineral.log`。

### 元数据 / 文档

- README:特性 / 构建 / 运行 / 登录 / XDG 路径 / 全套快捷键 / 架构 / 开发命令。
- ROADMAP:6 条长远方向(client/server / AI agent / Lua / 本地音乐 / 多 channel / 插件)。
- CLAUDE.md:codebase 约定 + lint 政策 + 体量约束 + 容易踩的坑。
- workspace 元数据:license = MIT / repository / authors / rust-version 一处定义,所有 crate `.workspace = true` 继承。
- per-crate description 补全。

### 已知不做(本期)

- gapless playback(rodio 上游限制)
- 多源在线 search lane(本地过滤够用)
- AuthRefresh lane(cookie 过期 UI 静默)
- 歌词翻译 / 罗马音切换 UI(字段就绪,UI 缺切换入口)
- plays 列接真值(等本地持久化基建)
- LocalScan + 本地 channel + .ncm 解码(等持久化基建)
