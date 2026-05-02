# Changelog

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/),版本号遵循 [Semver](https://semver.org/lang/zh-CN/)。

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
