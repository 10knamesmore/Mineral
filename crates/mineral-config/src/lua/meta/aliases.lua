-- 配置值类型

-- 终端色槽，名称或编号 0-15
---@alias mineral.AnsiSlot "black"|"red"|"green"|"yellow"|"blue"|"magenta"|"cyan"|"white"|"bright_black"|"bright_red"|"bright_green"|"bright_yellow"|"bright_blue"|"bright_magenta"|"bright_cyan"|"bright_white"|integer

-- 颜色："#rrggbb"、{ hex = "#rrggbb" }、{ ansi = "blue" } 或 { reset = true }
-- ansi 跟随终端配色，reset 使用终端默认色。
---@alias mineral.ColorValue string|{ hex: string }|{ ansi: mineral.AnsiSlot }|{ reset: boolean }

-- 具体色或主题色引用，如 "peach" 或 { token = "peach" }
---@alias mineral.ColorRef mineral.ColorValue|{ token: string }

-- 单键或键数组，如 "<Space>" 或 { "n", "j" }；数组整体替换，空数组解绑。
--
-- 键语法(nvim 表示法):
-- - 单字符键原样写:"j"、"/"、"+";**大小写有别**,"J" 即 Shift+j,不必写 "<S-j>"。
-- - 特殊键用尖括号(键名大小写不敏感):"<Space>" / "<Tab>" / "<CR>"(或 <Enter>
--   / <Return>)/ "<Esc>" / "<BS>" / "<Left>" / "<Right>" / "<Up>" / "<Down>"。
-- - 修饰前缀:"<S-Left>"、"<C-x>"、"<C-S-Right>";仅支持 C-(Ctrl)/ S-(Shift),
--   且 S- 只对非字符键有意义(字符键的 Shift 已编码在字符本身)。
-- - <A->(Alt)/ F1-F12 / Home / End / PageUp 等暂不支持。
---@alias mineral.KeyBinding string|string[]

---来源名，未加载的来源跳过
---@alias mineral.SourceName "netease"|"bilibili"|"local"|"mineral"|string

---搜索目标类型
---@alias mineral.SearchKind "song"|"album"|"artist"|"playlist"|"user"

---音质档位，由低到高
---@alias mineral.BitRate "standard"|"higher"|"exhigh"|"lossless"|"hires"

---时间格式：clock 为分秒（满小时显示时分秒），seconds 为总秒数。
---pattern 支持 {h}{hh}{m}{mm}{s}{ss}；分秒为 0-59，双写补零。
---@alias mineral.TimeFormat "clock"|"seconds"|{ pattern: string }

---菜单横向对齐；数字 0 贴左、0.5 居中、1 贴右。
---@alias mineral.MenuAlign "left"|"center"|"right"|number

---统计保留天数，正整数或 false（永久）
---@alias mineral.RetentionDays false|integer

---歌单列表的呈现策展函数:只管呈现(挑选 / 命名 / 排序),动不了数据。
---收歌单投影数组,返回要展示的条目:**省略 = 隐藏,顺序 = 展示序**,
---`name` / `description` 改了即覆盖(其余字段改动忽略,`id` 是只读身份键)。
---函数报错 / 超时 / 返回非法形态一律原列表透传(歌单不会因脚本 bug 消失)。
---@alias mineral.CuratePlaylistsFn fun(lists: mineral.PlaylistBrief[]): mineral.PlaylistBrief[]

---标题片段，icon、field、text 三选一
---@class mineral.TitleSegment
---@field icon? boolean 显示当前状态图标
---@field field? mineral.TitleField 引用字段
---@field text? string 固定文本
---@field prefix? string 字段前缀；字段为空时连同前后缀隐藏
---@field suffix? string 字段后缀
---@field format? mineral.TimeFormat 时间格式；省略用 clock，非时间字段忽略
