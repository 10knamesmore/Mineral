# Mineral 配置指南

一个文件管所有:`~/.config/mineral/config.lua`。本文按命名空间逐段讲每个旋钮是什么、默认值多少、什么时候该动它;编辑器内的权威字段注解是 `mineral config init` 生成的类型 stub(装了 [lua-language-server](https://github.com/LuaLS/lua-language-server) 即有补全 / 类型检查 / 悬浮文档)。

## 基本规则

```bash
mineral config init    # 生成 config.lua 模板 + default.lua 参考 + LSP 类型注解
mineral config check   # 离线校验(语法 / 未知字段 / 类型)
```

- **只写想改的**:用户配置与默认值**深合并**——省略的字段全部回落默认。例外:数组值(键绑定)是**整体替换**,不逐元素合并
- **写错不会崩**:类型不对 / 未知字段 / 超出取值集合 → 整份配置回落默认 + 启动告警
- `default.lua` 是同目录生成的完整默认值参考,**程序不读它**,改它无效
- 文件同时是脚本:顶层 `mineral.*` 调用(`return` 之前)是真实生效的脚本,见[脚本指南](./scripting.md)

### 生效方式

| 改动 | 生效 |
|---|---|
| `tui.theme` / `tui.keys` / `tui.behavior`、顶层脚本 | **保存即热重载** |
| 其余 `tui.*`(动画时长 / 布局 / 频谱 / 封面…) | 重启 TUI |
| `audio` / `cache` / `download` / `sources` / `daemon` / `script` | 重启 daemon(默认退出 TUI 会带走自拉起的 daemon,重开即生效;daemon 续命时需手动重启) |

### 环境变量(优先于配置文件)

| 变量 | 覆盖 |
|---|---|
| `MINERAL_AUDIO_NULL=1` | `audio.backend = "null"`(强制无声;headless / 测试用) |

---

## tui.theme — 主题色板

14 个颜色 token + 3 个语义角色。色值一律 `"#rrggbb"` 六位十六进制(必须带 `#`)。默认主题是 [Catppuccin Mocha](https://catppuccin.com/);想整体换主题,把 14 个 token 全写一遍即可。

| token | 默认 | 用在哪 |
|---|---|---|
| `base` | `#1e1e2e` | 主背景 |
| `mantle` | `#181825` | 次背景:嵌套面板 / 浮层底 |
| `crust` | `#11111b` | 第三背景:底部 transport 条 |
| `surface0` | `#313244` | 行选中背景 / 进度条轨道 |
| `surface1` | `#45475a` | 未聚焦边框 / 分隔线 |
| `overlay` | `#6c7086` | 暗淡文本 / 二级标签 |
| `subtext` | `#a6adc8` | 三级文本 / metadata |
| `text` | `#cdd6f4` | 主文本 |
| `accent` | `#cba6f7` | 主强调:选中 / 聚焦边框 / 在播标记 |
| `accent_2` | `#74c7ec` | 副强调:进度条填充 / 频谱顶段 |
| `red` | `#f38ba8` | 错误 / 删除 / love 标记 |
| `yellow` | `#f9e2af` | 暂停指示 |
| `green` | `#a6e3a1` | 播放指示 |
| `peach` | `#fab387` | 命令 / 搜索前缀 |

`roles` 把「语义角色」映射到 token 名(来源徽标等只声明角色,不直接点名颜色),取值必须是上面 14 个 token 名之一:

```lua
roles = { accent = "red", muted = "subtext", faint = "overlay" }   -- 默认
```

## tui.keys — 键位重映射

方向是「**动作 → 键**」:给某动作写新键即完全替换其默认键;写空数组 `{}` 解绑。两个动作绑同一键时后写的生效(不报错,自己留意)。

**键语法**(nvim 表示法):

- 单字符键原样写:`"j"`、`"/"`、`"+"`;**大小写有别**,`"J"` 即 Shift+j
- 特殊键用尖括号:`"<Space>"` `"<Tab>"` `"<CR>"`(或 `<Enter>`)`"<Esc>"` `"<BS>"` `"<Left>"` 等
- 修饰前缀:`"<C-x>"`(Ctrl)、`"<S-Left>"`(Shift,只对非字符键有意义)、可组合 `"<C-S-Right>"`
- 值可以是单键或键数组:`activate = { "l", "<CR>" }`(数组**整体替换**默认绑定)
- 暂不支持:`<A->`(Alt)、F1-F12、Home/End/PageUp

| 动作 | 默认键 | 说明 |
|---|---|---|
| `play_pause` | `<Space>` | 暂停 / 恢复 |
| `next` / `prev` | `n` / `p` | 下一首 / 上一首(分界见 `daemon.prev_restart_threshold_ms`) |
| `toggle_fullscreen` | `z` | 进 / 退全屏播放态 |
| `open_queue` | `<Tab>` | 播放队列浮层(再按关闭) |
| `quit` | `q` | 退出确认 |
| `cycle_lyric` | `t` | 歌词副轨:原文 → 翻译 → 罗马音 |
| `enter_search` | `/` | 搜索输入(全屏态屏蔽) |
| `activate` | `l`、`<CR>` | 进入歌单 / 播放选中曲 |
| `back` | `h`、`<Esc>`、`<BS>` | 返回上级 / 清搜索词 |
| `cycle_mode` | `m` | 循环播放模式 |
| `volume_up` / `volume_down` | `+`、`=` / `-`、`_` | 音量增减(步长见 behavior) |
| `seek_forward` / `seek_backward` | `<Right>` / `<Left>` | 快进退(步长见 behavior) |
| `seek_forward_big` / `seek_backward_big` | `<S-Right>` / `<S-Left>` | 大步快进退 |
| `move_down` / `move_up` | `j`、`<Down>` / `k`、`<Up>` | 列表光标移动 |
| `move_down_big` / `move_up_big` | `J` / `K` | 大步移动(行数见 behavior) |
| `move_first` / `move_last` | `g` / `G` | 跳首 / 末行 |
| `love` | `f` | 切换选中曲 ♥ |
| `download` | `d` | 下载选中曲 / 歌单 |
| `script` | `{}` | 脚本动作绑定:`mineral.action` 注册名 → 键,如 `script = { ["my.skip_short"] = "X" }` |

两个**硬编码键不在表内、不可重映射**(任何配置下都存在的逃生口):

- `<C-c>` — 立即退出 TUI(跳过动画与确认;不动 daemon)
- `Q`(Shift+q)— 退出 TUI **并停止 daemon**(不确认;无视 `kill_spawned_daemon_on_exit`,attach 的 daemon 也停)。搜索输入态例外:大写 Q 是搜索词

## tui.behavior — 交互手感

| 字段 | 默认 | 说明 |
|---|---|---|
| `volume_step` | 5 | 单次音量增减,百分点 |
| `seek_step_secs` | 5 | 单次 seek 步长,秒 |
| `seek_big_step_secs` | 30 | 大步 seek(Shift),秒 |
| `list_jump_rows` | 7 | 列表大步跳行数(`J`/`K`) |
| `kill_spawned_daemon_on_exit` | `true` | 退出 TUI 连带关掉自己拉起的 daemon;`false` = daemon 续命后台播放,下次启动自动接回。只影响本次亲手拉起的 daemon,attach 已有 daemon 不杀(想连 daemon 一起退用 `Q`,它无视本旋钮) |

## tui.spectrum — 频谱面板

条高单位是 1/8 字符格,满高 64(8 行 × 8)。所有时长旋钮均为毫秒,按 `animation.frame_tick_ms` 折算成拍、与帧率解耦。

条高动态是效果器 **ADSR** 模型:attack 起音(上升)/ decay 衰减(播放中余韵)/ release 释音(暂停落 0);sustain = FFT 实时值本身,无旋钮。

**DSP 标定**(听感不对再动):

| 字段 | 默认 | 说明 |
|---|---|---|
| `fft_size` | 4096 | FFT 窗,样本数,建议 2 的幂。大 = 低频细节多但瞬态钝、起播首窗慢;小 = 跟手但低频糊。**外键:`audio.tap_capacity` 须 ≥ 2 × 此值** |
| `f_min` / `f_max` | 20 / 20000 | 频率轴上下界,Hz;`f_max` 超采样率一半时自动取一半 |
| `log_axis_blend` | 0.92 | 频率轴对数化 0-1:1 = 纯对数(每 octave 等宽);略小于 1 可收掉低频「宽平顶」 |
| `db_floor` / `db_ceil` | -65.0 / -6.0 | dB 标定下 / 上界,共同决定显示动态范围。抬 floor = 砍安静细节整体变矮 |
| `peak_mix` | 0.5 | 频带统计峰值占比 0-1:0 = 纯均值(平),1 = 纯峰值(躁) |

**观感**(放心乱调):

| 字段 | 默认 | 说明 |
|---|---|---|
| `show_peak_cap` | `true` | 条顶 ▔ 浮标 |
| `show_trail` | `true` | peak 与条之间的余韵渐隐 |
| `hue_rotate` | `true` | 无封面色时整体色相缓慢漂移 |
| `spring_peak` | `true` | peak 弹簧物理(过冲 + 回弹);`false` = 直接吸附 |
| `baseline_min` | 3 | 任何状态下条的最小高度(1/8 格,0-64);0 = 静默时面板全空 |

**ADSR / peak 时长**(毫秒):

| 字段 | 默认 | 说明 |
|---|---|---|
| `attack_ms` | 50 | 起音:条高上升 90% 到位;越小越贴鼓点,≤帧间隔则瞬时 |
| `decay_ms` | 100 | 衰减:播放中余韵滑落;比 attack 大才有「快攻慢放」的动画感 |
| `release_ms` | 200 | 释音:暂停 / 无信号时落向 baseline |
| `peak_hold_ms` | 192 | 新 peak 原位悬停时长 |
| `peak_fall_ms` | 512 | 悬停结束后 peak 从满高落到 0 的满程时长 |

**配色 / 弹簧**:

| 字段 | 默认 | 说明 |
|---|---|---|
| `hue_cycle_ms` | 30000 | 色相转满一圈(360°)的时长 |
| `cover_fade_ms` | 6000 | 封面取色就绪后,从当前配色缓动到封面色场的时长 |
| `cover_vshift_permille` | 200 | 封面色场顶端沿色带的纵向偏移(‰,0-1000);拉开条底 / 条顶明度层次 |
| `spring_stiffness` | 0.35 | 弹簧刚度;0.1-1.0 合理,太大瞬间过冲像 bug |
| `spring_damping` | 0.45 | 弹簧阻尼;< 2√刚度 时欠阻尼有回弹感,越大越稳越不弹 |

## tui.cover — 封面管线

抓取 → 解码缩放 → 磁盘缓存 → k-means 取色喂频谱。

| 字段 | 默认 | 说明 |
|---|---|---|
| `http_timeout_secs` | 30 | 单张封面下载超时,秒 |
| `max_dim` | 384 | 解码后等比缩放到的最大边,px;终端显示足够,大了费内存 |
| `storage` | `"resized"` | 磁盘缓存存什么:`"raw"` = 原始字节(无损,体积大);`"resized"` = 缩放后重编码 JPEG(省盘,命中只解小图,CPU 大降) |
| `jpeg_quality` | 100 | 重编码质量 1-100;仅 `storage = "resized"` 时生效 |
| `debounce_ms` | 80 | 列表滚动停稳多久才渲染真图;期间显示程序化色块占位 |
| `download_workers` | 4 | 封面下载并发 worker 数 |
| `encode_workers` | 2 | 终端图片协议编码并发 worker 数 |

`kmeans` 子表(取色;取出的色不满意再动):

| 字段 | 默认 | 说明 |
|---|---|---|
| `sample_dim` | 64 | 取色采样边长;64² ≈ 4 千像素,够聚类、极省 CPU |
| `swatches` | 6 | 重点色上限(聚类 k);色多层次细、色少更整体 |
| `seed` | `0x5EEDC0DE` | 聚类种子,**必须固定**,否则同一封面每次取色不同 |
| `max_iter` / `converge` | 20 / 5.0 | 迭代上限 / 收敛阈值(Lab 空间) |
| `l_min` / `l_max` | 8.0 / 92.0 | 丢弃近黑 / 近白像素的明度界(Lab L),防黑白背景霸占色板 |
| `chroma_min` | 8.0 | 丢弃近灰像素的彩度下限 |
| `min_valid_pixels_pct` | 5 | 过滤后有效像素低于此(%)改用全部像素,保黑白封面有色 |

## tui.prefetch — 预取

提前抓即将看到的数据,用网络 / 内存开销换流畅度。

| 字段 | 默认 | 说明 |
|---|---|---|
| `radius` | 64 | 列表选中行上下各预取条数(封面 + 歌单曲目) |
| `playback_cover_radius` | 3 | 沿播放队列给在播曲前后各预取几张封面,服务自动切歌 |
| `play_count_debounce_ms` | 500 | 选中停留超过此毫秒才查远端播放次数,防翻列表打满 API |
| `prewarm_ahead` | 1 | 全屏稳态提前编码后几首封面,消自动切歌的占位闪 |

## tui.lyrics — 歌词面板

| 字段 | 默认 | 说明 |
|---|---|---|
| `fullscreen_line_gap` | 1 | 全屏沉浸态行间空行数;0 = 紧排但滚动变瞬跳 |
| `compact_line_gap` | 0 | 非全屏紧凑态行间空行数 |
| `scroll_ms` | 280 | 切行整列平移 + 高亮交叉淡入的过渡时长;超过此窗口直接吸附 |

两个 `*_line_gap` 可被脚本 `mineral.ui.override` 做 session 级覆盖(见[脚本指南](./scripting.md))。

## tui.animation — 动画

各时长均为毫秒,运行时按 `frame_tick_ms` 折算成拍数(四舍五入、至少一拍);设 0 ≈ 关闭该动画(一帧到位)。**改 `frame_tick_ms` 不改各动画的真实时长**——时长与帧率解耦。

| 字段 | 默认 | 说明 |
|---|---|---|
| `frame_tick_ms` | 16 | 主循环帧间隔;16 ≈ 60fps,越小越流畅越费 CPU,是所有 `*_ms` 折算的分母 |
| `transition_ms` | 288 | 启动扩大 / 退出收缩整屏转场(以光标真实位置为缩放锚点) |
| `sweep_ms` | 288 | 侧栏 歌单↔曲目 切换扫入 |
| `fullscreen_ms` | 288 | 全屏播放态进退场形变 |
| `popup_anim_ms` | 288 | 浮层(队列 / 确认框)弹出收起 |
| `toast_anim_ms` | 96 | 顶栏通知横向展开收起 |
| `view_sweep` | `"push"` | 侧栏切换风格:`"push"` = 新旧视图一起平移;`"cover"` = 新视图从右盖上 |

## tui.toast — 顶栏通知

| 字段 | 默认 | 说明 |
|---|---|---|
| `flash_ttl_secs` | 4 | 一次性通知(下载完成 / 配置告警等)停留秒数 |

## tui.layout — 布局

单位是终端字符格:宽 = 列数,高 = 行数。

| 字段 | 默认 | 说明 |
|---|---|---|
| `min_full_width` / `min_full_height` | 80 / 24 | 终端小于此尺寸退紧凑布局(无歌词 / 频谱面板) |
| `fs_left_pct` | 44 | 全屏左栏(封面 + transport)占宽 %,余下归歌词 |
| `fs_spectrum_height` | 14 | 全屏底部频谱通栏高,行 |
| `fs_transport_height` | 8 | 全屏 transport 条高,行(内容 6 + 边框 2) |
| `dock_w_pct` | 36 | 停靠浮层(播放队列)占屏宽 % |

---

## audio — 音频引擎

daemon 持有,改后重启 daemon。

| 字段 | 默认 | 说明 |
|---|---|---|
| `volume` | 100 | 启动初始音量 %;运行期音量不落盘,每次启动回到此值 |
| `backend` | `"auto"` | `"auto"` = 打开默认声卡,失败自动降级无声空跑;`"null"` = 强制无声。环境变量 `MINERAL_AUDIO_NULL` 优先 |
| `playback_quality` | `"exhigh"` | 在线播放音质:`standard / higher / exhigh / lossless / hires`;源没有对应档会回落 |
| `engine_tick_ms` | 20 | 引擎主循环节拍;影响 seek / 停止响应延迟,不建议动 |
| `prefetch_bytes` | 256 KiB | 流式起播前预拉字节;大 = 起播慢但 seek 命中缓冲概率高 |
| `tap_capacity` | 8192 | 频谱 PCM 环形缓冲,样本数。**须 ≥ 2 × `tui.spectrum.fft_size`**,否则 UI 卡一帧就丢样本出毛刺 |

## cache — 磁盘缓存容量

LRU,满了自动驱逐;改小不立刻删文件,下次写入时驱逐。可写算式。

| 字段 | 默认 | 说明 |
|---|---|---|
| `audio_capacity` | `10 * 1024 ^ 3`(10 GiB) | 音频本体缓存上限,字节 |
| `cover_capacity` | `4 * 1024 ^ 3`(4 GiB) | 封面缓存上限,字节 |

## download — 下载导出

永久导出,不受缓存容量约束。

| 字段 | 默认 | 说明 |
|---|---|---|
| `quality` | `"lossless"` | 下载音质,与播放音质相互独立 |
| `dir` | `nil`(= `~/Music/mineral`) | 导出目录,绝对路径 |

## sources.netease — 网易云源

| 字段 | 默认 | 说明 |
|---|---|---|
| `timeout_secs` | 100 | 单次 API 请求超时,秒 |
| `proxy` | `false` | `false` = 不走代理;字符串 = 代理 URL(如 `"socks5://127.0.0.1:1080"`) |
| `max_connections` | 0 | 到源的最大并发连接,0 = 不限 |

## daemon — 后端节拍

多为内部时序参数,默认值经过调校,**没有明确诉求不要动**;改后重启 daemon。

| 字段 | 默认 | 说明 |
|---|---|---|
| `gapless_prefetch_ms` | 10000 | 距曲尾多少毫秒开始预排下一曲(无缝窗口);太小可能退化出间隙 |
| `prev_restart_threshold_ms` | 3000 | `p` 键分界:进度超过此值回曲首,否则上一首 |
| `player_tick_ms` | 20 | 播放核心后台循环间隔;影响自动切歌 / 事件转发延迟 |
| `session_save_secs` | 15 | 播放进度周期落盘节流,秒;切歌等另有即时落盘 |
| `heartbeat_secs` | 180 | 状态心跳日志间隔,秒;daemon 与 TUI 各打一条供排查 |
| `report_interval_ms` | 200 | 向系统媒体控件(MPRIS / Now Playing)上报进度的间隔 |
| `seek_threshold_ms` | 1000 | 进度偏离线性预期超过此值判定为 seek(供媒体控件上报) |
| `download_speed_tick_ms` | 150 | 下载测速刷新节流 |
| `channel_workers_per` | 8 | 每个音乐源的后台并发 worker;大 = 抓取快但易撞源限流 |

## script — 脚本运行时

config.lua 顶层 `mineral.*` 调用的运行时参数,详见[脚本指南](./scripting.md)。

| 字段 | 默认 | 说明 |
|---|---|---|
| `watchdog_instruction_interval` | 2000 | 每多少条 Lua VM 指令查一次墙钟;小 = 灵敏但开销大 |
| `watchdog_soft_wall_ms` | 100 | 回调超此时长记 warn,继续跑 |
| `watchdog_hard_wall_ms` | 1000 | 回调超此时长被中断(只杀本次调用,脚本仍存活) |
| `hook_timeout_ms` | 2000 | 拦截 hook 软超时;超时按放行处理,不卡播放 |
| `spawn_max_concurrent` | 8 | `mineral.spawn` 子进程并发上限;0 = 不限 |
