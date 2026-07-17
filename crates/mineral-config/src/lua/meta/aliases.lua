-- 手写类型别名:值语法是联合形态(字符串 | 表 | 布尔等混写),无法从单一
-- Rust 类型投影,与 Rust 侧自定义 Deserialize 一一对应,改值语法两边同步。

-- 终端 ANSI 槽:具名或编号(0-15,数字下标即槽号)。
---@alias mineral.AnsiSlot "black"|"red"|"green"|"yellow"|"blue"|"magenta"|"cyan"|"white"|"bright_black"|"bright_red"|"bright_green"|"bright_yellow"|"bright_blue"|"bright_magenta"|"bright_cyan"|"bright_white"|integer

-- 具体颜色值,三种写法:
-- - "#rrggbb" 简写(六位十六进制,必须带 `#`,如 "#cba6f7"),或等价的 { hex = "#rrggbb" };
-- - { ansi = "blue" } / { ansi = 4 }:终端 16 个 ANSI 槽之一,
--   实际 RGB 由终端当前配色决定——引用即"跟随终端";
-- - { reset = true }:终端默认前景 / 背景(不指定具体色)。
---@alias mineral.ColorValue string|{ hex: string }|{ ansi: mineral.AnsiSlot }|{ reset: boolean }

-- 颜色引用:具体色值(写法同 mineral.ColorValue),或指向 14 个主题 token 之一
-- (裸 token 名如 "peach",或 { token = "peach" },随主题联动)。
---@alias mineral.ColorRef mineral.ColorValue|{ token: string }

-- 键绑定值:单键字符串(如 "<Space>")或键数组(如 { "n", "j" });数组**整体替换**默认绑定。
--
-- 键语法(nvim 表示法,`KeyChord` 解析):
-- - 单字符键原样写:"j"、"/"、"+";**大小写有别**,"J" 即 Shift+j,不必写 "<S-j>"。
-- - 特殊键用尖括号(键名大小写不敏感):"<Space>" / "<Tab>" / "<CR>"(或 <Enter>
--   / <Return>)/ "<Esc>" / "<BS>" / "<Left>" / "<Right>" / "<Up>" / "<Down>"。
-- - 修饰前缀:"<S-Left>"、"<C-x>"、"<C-S-Right>";仅支持 C-(Ctrl)/ S-(Shift),
--   且 S- 只对非字符键有意义(字符键的 Shift 已编码在字符本身)。
-- - <A->(Alt)/ F1-F12 / Home / End / PageUp 等暂不支持。
---@alias mineral.KeyBinding string|string[]

---来源名:内置源有补全,插件源写任意 string 也合法(没加载的名字运行时静默跳过)。
---@alias mineral.SourceName "netease"|"bilibili"|string

---channel 搜索的目标类型(封闭集合,typo 加载期报错)。
---@alias mineral.SearchKind "song"|"album"|"artist"|"playlist"|"user"

---音质档位,低→高;播放音质与下载音质各自独立引用本集合。
---@alias mineral.BitRate "standard"|"higher"|"exhigh"|"lossless"|"hires"

---时间字段渲染格式。"clock" = mm:ss(>=1h 进 h:mm:ss);"seconds" = 总秒数;
---{ pattern = "..." } = 占位串 {h}{hh}{m}{mm}{s}{ss}(最细到秒)。
---@alias mineral.TimeFormat "clock"|"seconds"|{ pattern: string }

---弹出菜单相对锚点行的横向对齐:关键字,或 0.0~1.0 数字精确指定比例
---(0 贴左 / 0.5 居中 / 1 贴右)。
---@alias mineral.MenuAlign "left"|"center"|"right"|number

---埋点数据保留窗:`false` = 永久保留;正整数 = 保留天数(到期后台清理)。
---@alias mineral.RetentionDays false|integer

---歌单列表的呈现策展函数:只管呈现(挑选 / 命名 / 排序),动不了数据。
---收歌单投影数组,返回要展示的条目:**省略 = 隐藏,顺序 = 展示序**,
---`name` / `description` 改了即覆盖(其余字段改动忽略,`id` 是只读身份键)。
---函数报错 / 超时 / 返回非法形态一律原列表透传(歌单不会因脚本 bug 消失)。
---@alias mineral.CuratePlaylistsFn fun(lists: mineral.PlaylistBrief[]): mineral.PlaylistBrief[]

---窗口标题模板的一个段。三种形态互斥,按 key 自动识别(Rust 侧 untagged 枚举)。
---@class mineral.TitleSegment
---@field icon? boolean `true` 表示当前态状态图标(字形取自 icons)
---@field field? mineral.TitleField 引用字段
---@field text? string 固定字面文本
---@field prefix? string `field` 段在字段值前附加的文本;字段为空时整段(含 prefix/suffix)折叠
---@field suffix? string `field` 段在字段值后附加的文本
---@field format? mineral.TimeFormat `position`/`duration` 的渲染格式(非时间字段忽略)
