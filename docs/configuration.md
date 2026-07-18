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
| `MINERAL_SOCKET_DIR=<绝对路径>` | IPC socket 目录(非 config 字段,直接改运行期推导;优先于 `XDG_RUNTIME_DIR` / `$TMPDIR`)。socket 路径过长报错时用它指一个更短目录,测试隔离也用它 |

---

## tui.theme — 主题色板

14 个颜色 token + 3 个语义角色。每个 token 的色值可以是固定色 `"#rrggbb"`、终端 ANSI 槽(跟随终端配色)或终端默认——写法详见下方[色值写法](#色值写法)。默认主题是 [Catppuccin Mocha](https://catppuccin.com/)(全固定色);想整体换主题,把 14 个 token 全写一遍即可。

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

`search_hit` 子表是搜索命中字符的高亮样式(叠加在所在列的基础样式之上):

| 字段 | 默认 | 说明 |
|---|---|---|
| `color` | `"peach"` | 命中字符色:14 个 token 名之一(随主题联动),或任意[色值写法](#色值写法) |
| `modifiers` | `["bold", "underline", "italic"]` | 叠加的字体效果,数组**整体替换**;可选 `bold` / `italic` / `underline` / `dim` / `reversed` / `crossed_out`;空数组 = 仅变色 |

`dynamic` 子表是封面驱动的动态主题:在播封面取色就绪后,`accent` / `accent_2` 从当前可见色渐变到封面派生色(主色取封面最鲜艳的一簇、副色与主色拉开色相,明度会整形进深色背景上可读的区间),聚焦边框 / 进度条 / 在播标记等所有强调处全局联动;切到无封面的歌 / 取色失败时同样渐变回上表的静态 token,不闪跳:

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 关闭即恒用静态 `accent` / `accent_2` |
| `fade_ms` | `3000` | 切歌 / 封面就绪后 accent 渐变过去的时长,毫秒 |

### 色值写法

一个色值有四种写法(前两种是固定色,后两种把颜色交给终端决定):

| 写法 | 含义 |
|---|---|
| `"#rrggbb"` | 固定色简写(必须带 `#`) |
| `{ hex = "#rrggbb" }` | 固定色(结构化写法,同上) |
| `{ ansi = "blue" }` / `{ ansi = 4 }` | 引用终端 16 个 ANSI 槽之一,实际颜色由你的终端配色决定(换终端主题它跟着变) |
| `{ reset = true }` | 终端默认前景 / 背景 |

ANSI 槽名(或 `0`–`15` 编号):`black` `red` `green` `yellow` `blue` `magenta` `cyan` `white`(0–7)、`bright_black` … `bright_white`(8–15)。

> `search_hit.color` / `roles` / 来源徽标色除以上写法外,还能直接写 token 名(如 `"peach"`)引用某个主题 token;14 个 token 本身不能互相引用(它们**被**引用,不去引用别人)。

### Recipe:跟随终端配色

让**背景层**跟随终端当前配色(换终端主题界面底色跟着变、与编辑器 / tmux 同色),**强调层**保留固定色:

```lua
theme = {
  -- 背景层:跟随终端
  base     = { reset = true },          -- 主背景用终端默认背景
  mantle   = { ansi = "black" },        -- 嵌套面板底
  crust    = { ansi = "black" },        -- transport 条
  surface0 = { ansi = "bright_black" }, -- 行选中底
  surface1 = { ansi = "bright_black" }, -- 未聚焦边框
  -- 其余 token(accent / accent_2 / red / green / …)不写 = 沿用默认固定色
}
```

**为什么强调层别跟随**:频谱柱的纵向渐变、歌词的距离淡出 / 逐字扫染、进度条的 `red→green` 都是在两个颜色之间算中间色,要求应用手里有确切 RGB。ANSI 槽的实际颜色只有终端知道,应用**算不出中间色**,会退成「过半硬切」的两段色(不崩、但糙)。所以喂给这些渐变的强调 token(`accent` / `accent_2` / `subtext` / `overlay`、进度条的 `red` / `green`)留固定色才顺滑;纯做背景 / 边框的 token 跟随终端零损失。上面把 `surface0` 也跟随了终端是有意折中——它主要是行选中底色,只有频谱余韵 / 歌词远行的淡出会用到它作终点,那点淡出变糙基本看不出,想更严谨可把它也留固定色。封面取色不受影响——那些色是从专辑图抠出的真 RGB。

### Recipe:多主题合集 + 启动随机

`config.lua` 本身是 Lua 脚本(`return` 前可写任意代码),`tui.theme` 的值就是一张表——所以可以维护多套调色板、每次加载随机挑一套。把调色板放独立文件(纯数据),config 里 `dofile` 取用:

```lua
-- ~/.config/mineral/themes.lua —— 返回一张表,每套 14 个 token
return {
  tokyonight = { base = "#1a1b26", accent = "#bb9af7", --[[ …其余 12 个… ]] },
  gruvbox    = { base = "#282828", accent = "#fabd2f", --[[ … ]] },
  -- 也可混固定色与跟随终端:
  follow     = { base = { reset = true }, accent = { ansi = "magenta" }, --[[ … ]] },
}
```

```lua
-- ~/.config/mineral/config.lua
local THEMES = dofile((os.getenv("HOME") or "") .. "/.config/mineral/themes.lua")

-- 每次配置加载随机挑一套
math.randomseed(os.time())
local pool = { "tokyonight", "gruvbox", "follow" } -- 想参与随机的名字
local chosen = THEMES[pool[math.random(#pool)]]

return {
  tui = { theme = chosen }, -- 固定某套就写 THEMES.gruvbox
}
```

注意:配置**保存热重载**时也会重跑这段 → 每次存盘都重新随机。想「仅启动随机、编辑时不变」,把选中结果缓存到文件(如 `paths` 下)再读回即可。

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
| `enter_search` | `/` | 当前列表行内过滤搜索(全屏态屏蔽) |
| `open_search` | `s` | 打开搜索界面(在线搜索:歌曲 / 专辑 / 艺人 / 歌单);区别于 `/` 的本地过滤 |
| `activate` | `l`、`<CR>` | 进入歌单 / 播放选中曲 |
| `back` | `h`、`<Esc>`、`<BS>`、`<C-h>` | 返回上级 / 清搜索词(`<C-h>` 兼容把 `<BS>` 报成 Ctrl-h 的终端) |
| `drill_into` | `<C-l>` | 下探:进专辑 / 艺人详情页 |
| `cycle_detail_section` | `[`、`]` | 详情页分区切换(`[` 上一区 / `]` 下一区) |
| `cycle_mode` | `m` | 循环播放模式 |
| `volume_up` / `volume_down` | `+`、`=` / `-`、`_` | 音量增减(步长见 behavior) |
| `seek_forward` / `seek_backward` | `<Right>` / `<Left>` | 快进退(步长见 behavior) |
| `seek_forward_big` / `seek_backward_big` | `<S-Right>` / `<S-Left>` | 大步快进退 |
| `move_down` / `move_up` | `j`、`<Down>` / `k`、`<Up>` | 列表光标移动 |
| `move_down_big` / `move_up_big` | `J` / `K` | 大步移动(行数见 behavior) |
| `move_first` / `move_last` | `g` / `G` | 跳首 / 末行 |
| `love` | `f` | 切换选中曲 ♥ |
| `download` | `d` | 下载选中曲 / 歌单 |
| `open_action_menu` | `o` | 打开操作菜单(对选中曲 / 歌单) |
| `open_copy_menu` | `y` | 打开复制菜单(标题 / 艺人 / 链接 / 自定义模板,见 `tui.copy`) |
| `dismiss_notice` | `x` | 关最早一张驻留通知卡片(连按逐条关) |
| `scroll_line_down` / `scroll_line_up` | `<C-d>` / `<C-u>` | 逐行滚:全屏态滚歌词,浏览态滚列表视口(行数见 `behavior.line_scroll_rows`) |
| `scroll_page_down` / `scroll_page_up` | `<C-f>` / `<C-b>` | 翻页滚(行数见 `behavior.page_scroll_rows`) |
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
| `scrolloff` | 3 | 光标与列表视口上下边缘保持的最小行距(nvim `scrolloff`);光标在安全区内移动时视口不动,0 = 贴边才滚 |
| `line_scroll_rows` | 1 | 单行档滚动(`<C-d>`/`<C-u>`)一次行数,列表与全屏歌词共用 |
| `page_scroll_rows` | 15 | 翻页档滚动(`<C-f>`/`<C-b>`)一次行数 |
| `search_prefetch_rows` | 8 | 搜索结果懒分页预取半径:光标距已加载末行 ≤ 此行数且未榨干时自动拉下一页 |
| `kill_spawned_daemon_on_exit` | `true` | 退出 TUI 连带关掉自己拉起的 daemon;`false` = daemon 续命后台播放,下次启动自动接回。只影响本次亲手拉起的 daemon,attach 已有 daemon 不杀(想连 daemon 一起退用 `Q`,它无视本旋钮) |
| `remember_track_pos` | `"session"` | 歌单内光标位置记忆:`"off"` 不记 / `"session"` 本次运行内 / `"persist"` 整表落 `tui.db` 跨重启;搜索命中定位(`search.deep.locate_on_enter`)优先于记忆位置 |

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

## tui.waveform — 进度条波形

transport 进度条化身全曲振幅波形:播放头扫过的部分点亮,副歌 / 安静桥段的位置一眼可见,seek 有了参照物。

包络由 daemon 离线解码整曲算出、落库复用,只对**本地曲库 / 已缓存 / 已下载**的曲目可算——流播半截解不出全曲形状。包络未就绪时自动回落普通进度条,不占额外空间。首次流式播放的曲子在曲尾入缓存后包络就绪,**第二次播放起有波形**。

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 进度条是否化身振幅波形;包络未就绪自动回落普通进度条 |
| `cover_color` | `true` | 已播放段吃封面取色(与频谱同源);`false` 用主题 accent |
| `contrast` | `2.0` | 响度 → 条高的对比 gamma:`1` = 线性,越大安静段压得越低、起伏越明显。渲染层映射不动包络数据,改了即时生效 |
| `edge_radius` | `3` | 播放头软边半径(列):前后各此数列在已播色与轨道色之间插值,边界雾化溶解;`0` = 硬边(无播放头高亮,已播渐变的生长边缘即 seek 位置) |

### Recipe:全屏沉浸态才展开波形

「browse 态保持细进度条、全屏才上波形」这类场景化行为不设配置项——观察终端态、
用 override 覆盖开关即可(override 是配置合成的一层,翻转即热生效)。回落值提成
共享 local,config 树与 observe 回调两处同用一个变量:

```lua
local WAVEFORM_DEFAULT = false

mineral.observe("terminal", function(t)
  local on = (t ~= nil and t.fullscreen) or WAVEFORM_DEFAULT
  mineral.config.override({ tui = { waveform = { enabled = on } } })
end)

return {
  tui = { waveform = { enabled = WAVEFORM_DEFAULT } },
}
```

## tui.cover — 封面管线

抓取 → 解码缩放 → 磁盘缓存 → k-means 取色喂频谱。

| 字段 | 默认 | 说明 |
|---|---|---|
| `protocol` | `"auto"` | 终端图协议:`"auto"` 启动探测协商,已知「探测穿透、渲染不合成图数据」的环境(如 zellij)自动降级halfblock;`"halfblocks"` / `"kitty"` / `"sixel"` / `"iterm2"` 强制 |
| `http_timeout_secs` | 30 | 单张封面下载超时,秒 |
| `max_dim` | 384 | 解码后等比缩放到的最大边,px;终端显示足够,大了费内存 |
| `storage` | `"resized"` | 磁盘缓存存什么:`"raw"` = 原始字节(无损,体积大);`"resized"` = 缩放后重编码 JPEG(省盘,命中只解小图,CPU 大降) |
| `jpeg_quality` | 100 | 重编码质量 1-100;仅 `storage = "resized"` 时生效 |
| `debounce_ms` | 80 | 列表滚动停稳多久才渲染真图;期间显示程序化色块占位 |
| `download_workers` | 4 | 封面下载并发 worker 数 |
| `encode_workers` | 2 | 终端图片协议编码并发 worker 数 |

`kitty_transmit` 子表(kitty 图协议数据流式传输)。kitty graphics protocol 的图数据
传输与占位显示分离:编码就绪后把数 MB 的传输序列拆成完整转义单元、逐帧按预算发给
终端,首次显示只写几 KB 占位符——消掉「没播过的歌切过去卡一下」的首显传输尖峰。
仅对 kitty 协议生效;sixel / iTerm2 inline 图数据即显示,无从提前传输,此段无操作:

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 关闭则图数据在首次显示那一帧整段发送 |
| `per_tick_kb` | 768 | 每帧发送预算(KiB);越大传完越快,单帧终端解析负担越重 |

`cache` 子表(缓存预算;LRU,满了自动驱逐,磁盘项改小不立刻删文件):

| 字段 | 默认 | 说明 |
|---|---|---|
| `disk` | `4 * 1024 ^ 3`(4 GiB) | 磁盘缓存上限,字节 |
| `image` | `128 * 1024 ^ 2`(128 MiB) | 解码原图 RAM 预算;越界逐出最久未显示者 |
| `protocol` | `64 * 1024 ^ 2`(64 MiB) | 已编码终端协议(序列+源图副本)RAM 预算;越界逐出最久未渲染者 |

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

## tui.cover_transition — 全屏切歌封面转场

全屏沉浸页切歌瞬间,新旧封面以字符半块(halfblock)像素级合成一段转场,推满落定后
snap 回终端图协议高清。转场窗口恰好盖住新图的离线编码期,落定无占位闪。仅在新旧两图
都已在缓存时触发;缺任一图维持原行为(直接换图)。

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 关闭则切歌封面直接换,不做合成转场 |
| `style` | `"fade"` | `"fade"` 淡入淡出 / `"slide"` 旧左出新右进 / `"zoom"` 旧放大退场新回缩落定 |
| `duration_ms` | 900 | 转场时长 |

`zoom` 子表(`style = "zoom"` 时生效):

| 字段 | 默认 | 说明 |
|---|---|---|
| `scale` | 1.12 | 缩放幅度:旧图放大到此倍数退场,新图从此倍数回缩到 1;1 = 无缩放(等效 fade) |

## tui.ambient — 全屏氛围背景

全屏沉浸页整屏铺一层「当前封面 k-means 调色板」驱动的氛围渐变场:若干锚点色斑高斯
混合、缓慢漂移,切歌时调色板连续渐变到新封面,与动态 accent / 频谱色场同源同色。
只写背景色、不动前景文字;浓度随全屏形变进度淡入。ANSI / indexed 主题拿不到底色
分量,自动跳过。

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 关闭经同一渐变平滑淡出,不瞬跳 |
| `intensity` | 0.55 | 渐变场浓度 0-1:0 = 纯主题底色;过大前景对比度下降 |
| `sigma` | 0.26 | 锚点高斯半径(屏幕相对):越小色斑越聚拢,越大场越均匀 |
| `fade_ms` | 1400 | 切歌 / 取色就绪时调色板渐变到新封面的时长 |

`vignette` 子表(边缘暗角,保歌词 / 播控可读):

| 字段 | 默认 | 说明 |
|---|---|---|
| `strength` | 0.5 | 边缘向底色收敛的强度 0-1;0 = 关闭 |
| `inner` | 0.25 | 起始半径(到屏心相对距离,此内无暗角) |
| `outer` | 0.75 | 满强半径(屏角距离 ≈ 0.71,此值下角落近满强) |

`drift` 子表(锚点漂移):

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 关闭则场静止,仅切歌时颜色过渡 |
| `speed` | 1.0 | 速率倍率:1 = 各锚点自带角速度的原速 |
| `sway_pct` | 12.0 | 锚点绕锚位游走半径,屏幕尺寸的 %;太小则场几乎不变形 |

`rotate` 子表(颜色轮转:各锚点的色带采样位沿「暗 → 亮 → 暗」三角波往返,封面色在
色斑间缓慢流动;与 `drift` 正交——一个管颜色、一个管位置):

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 关闭则各锚点颜色钉在其 `pos` 位置 |
| `cycle_secs` | 30.0 | 一整个往返周期,秒;越小颜色流动越快 |

`anchors` 数组(**整体替换**,一项一个色斑):`x` / `y` 锚位(屏幕相对 0-1)、`pos`
色带采样位‰(0 = 色板最暗色,1000 = 最亮)、`speed_x` / `speed_y` 两轴角速度
(弧度/秒;两轴不同才游走出非正圆轨迹)、`phase_x` / `phase_y` 初始相位。默认五锚点
从暗到亮铺满色带,布局见 `default.lua`。

## tui.prefetch — 预取

提前抓即将看到的数据,用网络 / 内存开销换流畅度。

| 字段 | 默认 | 说明 |
|---|---|---|
| `radius` | 64 | 列表选中行上下各预取条数(封面 + 歌单曲目) |
| `playback_cover_radius` | 3 | 沿播放队列给在播曲前后各预取几张封面,服务自动切歌 |
| `play_count_debounce_ms` | 500 | 选中停留超过此毫秒才查远端播放次数,防翻列表打满 API |
| `prewarm_ahead` | 1 | 全屏稳态提前编码后几首封面,消自动切歌的占位闪 |

## tui.search — 搜索

`deep`(本地过滤)与 `channel`(远程搜索白名单)是两套互不相关的旋钮,共享这一张父表。

### `search.deep` —— 本地过滤搜索(`/`)

`/` 触发的本地过滤。`enabled` 打开后,Playlists 视图的搜索词会穿透到歌单内歌曲(歌名 / 艺人 / 专辑),进搜索态时后台补拉未缓存歌单的曲目,命中字符按 `theme.search_hit` 高亮。改后重启 TUI 生效(`theme.search_hit` 在 `tui.theme` 段下,保存即热重载)。

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | Playlists 视图搜索是否穿透到歌单内歌曲(总开关) |
| `weights.name` | 0.6 | 深度搜索歌名命中分折扣 0~1,0 = 该字段不参与匹配 |
| `weights.artist` | 0.5 | 艺人名命中分折扣(多艺人取最高,0 = 关闭) |
| `weights.album` | 0.4 | 专辑名命中分折扣(0 = 关闭) |
| `locate_on_enter` | `true` | 深度命中行 `Enter` 进歌单后光标是否直接定位到命中歌(`false` = 仍从头看) |

歌单最终分 = max(歌单名分, 歌单内最佳歌曲分);单曲分 = 各字段加权分取最高。

### `search.channel` —— channel 远程搜索白名单

Search 布局态两个下拉框(source / kind)的白名单:列出即暴露、顺序即下拉顺序,未列出的隐藏;空列表走消费侧防呆回退全量。

| 字段 | 默认 | 说明 |
|---|---|---|
| `sources` | `["netease", "bilibili"]` | source 下拉白名单+顺序;source 名开放(插件源可写),没加载的名字静默跳过 |
| `kinds` | `["song", "album", "artist", "playlist", "user"]` | kind 下拉白名单+顺序(封闭集合),与各 source 声明的可搜集合求交 |

## tui.lyrics — 歌词面板

| 字段 | 默认 | 说明 |
|---|---|---|
| `fullscreen_line_gap` | 1 | 全屏沉浸态行间空行数;0 = 紧排但滚动变瞬跳 |
| `compact_line_gap` | 0 | 非全屏紧凑态行间空行数 |
| `scroll_ms` | 280 | 切行整列平移 + 高亮交叉淡入的过渡时长;超过此窗口直接吸附 |
| `reattach_ms` | 4000 | 有时间戳歌手动滚走后空闲多久(毫秒)自动平滑回到跟随当前行;无时间戳歌不回锚 |
| `overshoot_damping` | 1 | 沉浸滚动撞首 / 末行的 rubber-band 过冲阻尼:过冲 = 超出行数 ÷ 此值,越大弹得越轻;0 视作 1 |
| `overshoot_max_permille` | 6000 | 单次过冲上限,行的千分比(6000 = 6 行);0 = 关闭边界回弹 |

任意配置字段(含两个 `*_line_gap`)可被脚本 `mineral.config.override` 做 session 级覆盖(见[脚本指南](./scripting.md))。

## tui.animation — 动画

各时长均为毫秒,运行时按 `frame_tick_ms` 折算成拍数(四舍五入、至少一拍);设 0 ≈ 关闭该动画(一帧到位)。**改 `frame_tick_ms` 不改各动画的真实时长**——时长与帧率解耦。

| 字段 | 默认 | 说明 |
|---|---|---|
| `frame_tick_ms` | 16 | 主循环帧间隔;16 ≈ 60fps,越小越流畅越费 CPU,是所有 `*_ms` 折算的分母 |
| `transition_ms` | 288 | 启动扩大 / 退出收缩整屏转场(以光标真实位置为缩放锚点) |
| `sweep_ms` | 288 | 侧栏 歌单↔曲目 切换扫入 |
| `list_scroll_ms` | 280 | 列表视口滚动平移(`<C-d>` 族与 `scrolloff` 触发的滚动) |
| `fullscreen_ms` | 288 | 全屏播放态进退场形变 |
| `popup_anim_ms` | 288 | 浮层(队列 / 确认框)弹出收起 |
| `toast_anim_ms` | 288 | 顶栏通知横向展开收起 |
| `focus_fade_ms` | 288 | 终端失焦/聚焦时顶栏变灰 + `◌ not focused` 徽标的淡入淡出(tmux 内需 `set -g focus-events on`;不支持 focus 事件的终端恒按聚焦渲染) |
| `view_sweep` | `"push"` | 侧栏切换风格:`"push"` = 新旧视图一起平移;`"cover"` = 新视图从右盖上 |
| `menu_reveal` | `"morph"` | 弹出菜单揭示风格:`"morph"` = 从锚点行形变而来;`"directional"` = 贴边方向性揭开 |
| `search_focus_transition` | `"slide"` | 搜索焦点高亮边框切换:`"slide"` = 从旧面板滑到新面板;`"instant"` = 瞬移直切 |
| `search_focus_morph_ms` | 240 | 搜索焦点高亮边框滑动时长(`search_focus_transition = "slide"` 时生效) |
| `spinner_frames` | braille 十帧 | loading 旋转占位帧(逐帧循环);空 `{}` = 只留文案不画字形 |

## tui.toast — 顶栏通知

| 字段 | 默认 | 说明 |
|---|---|---|
| `flash_ttl_secs` | 4 | 一次性通知(下载完成 / 配置告警等)停留秒数 |

## tui.copy — 复制菜单模板

`y` 复制菜单的自定义项,**追加**在内置项(标题 / 艺人 / … / URL)之后;`templates` 数组**整体替换**(默认 `{}`,无自定义项)。每项 `{ key?, label, template, context? }` 的 `template` 是函数,收实体表返回剪贴板文本,在 daemon 内的脚本运行时执行(超时 / 报错只 toast 不复制)。

| 字段 | 必填 | 说明 |
|---|---|---|
| `label` | 是 | 菜单项显示名 |
| `template` | 是 | `function(entity) -> string`,返回写入剪贴板的文本 |
| `key` | 否 | 快捷字母;与内置项同字母时顶掉其快捷位,省略 = 仅 `j`/`k` + `Enter` 可达 |
| `context` | 否 | `"song"`(默认,收 `mineral.Song`)/ `"playlist"`(收 `mineral.Playlist`,含 `songs`) |

```lua
copy = {
  templates = {
    { key = "f", label = "Copy full", template = function(s)
        return s.title .. " - " .. table.concat(s.artists, ", ")
      end },
  },
}
```

## tui.layout — 布局

单位是终端字符格:宽 = 列数,高 = 行数。

| 字段 | 默认 | 说明 |
|---|---|---|
| `min_full_width` / `min_full_height` | 80 / 24 | 终端小于此尺寸退紧凑布局(无歌词 / 频谱面板) |
| `fs_left_pct` | 44 | 全屏左栏(封面 + transport)占宽 %,余下归歌词 |
| `fs_spectrum_height` | 14 | 全屏底部频谱通栏高,行 |
| `fs_transport_height` | 8 | 全屏 transport 条高,行(内容 6 + 边框 2) |
| `dock_w_pct` | 36 | 停靠浮层(播放队列)占屏宽 % |
| `menu_align` | `"right"` | 弹出菜单相对锚点行的横向对齐:`"left"` / `"center"` / `"right"`,或 `0.0`~`1.0` 数字精确指定(0 贴左 / 0.5 居中 / 1 贴右) |

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

### audio.envelope — 响度包络计算

波形 seekbar(`tui.waveform`)用的全曲响度包络,daemon 离线解码整曲算出。默认值即
ITU-R BS.1770 规范参数,一般不用动。**参数变更只影响之后计算的包络,已落库的不自动
重算**(只有算法版本升级才触发全量重算)。

| 字段 | 默认 | 说明 |
|---|---|---|
| `points` | 200 | 包络定长点数;渲染端再按显示宽度二次重采样 |
| `block_ms` | 100 | 响度块时长(块内取均方) |
| `window_ms` | 400 | momentary 滑窗时长;4 × `block_ms` = BS.1770 momentary 的 75% 重叠窗 |
| `shelf.f0_hz` | 1681.97… | K-weighting 高频搁架转折频率(头部声学,~2kHz 以上 +4dB) |
| `shelf.gain_db` | 4.0 | 搁架增益 |
| `shelf.q` | 0.7072 | 搁架品质因数 |
| `shelf.band_exponent` | 0.4997 | 过渡带增益分配指数(`Vb = Vh^x`) |
| `highpass.f0_hz` | 38.14 | RLB 高通转折频率(人耳低频不敏感) |
| `highpass.q` | 0.5003 | 高通品质因数 |

## cache — 磁盘缓存容量

LRU,满了自动驱逐;改小不立刻删文件,下次写入时驱逐。可写算式。封面缓存预算见 `tui.cover.cache`。

| 字段 | 默认 | 说明 |
|---|---|---|
| `audio_capacity` | `10 * 1024 ^ 3`(10 GiB) | 音频本体缓存上限,字节 |

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
