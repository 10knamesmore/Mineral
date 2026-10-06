-- daemon 配置值类型

---统计保留天数，正整数或 false（永久）
---@alias mineral.RetentionDays false|integer

---歌单列表的呈现策展函数:只管呈现(挑选 / 命名 / 排序),动不了数据。
---收歌单投影数组,返回要展示的条目:**省略 = 隐藏,顺序 = 展示序**,
---`name` / `description` 改了即覆盖(其余字段改动忽略,`id` 是只读身份键)。
---函数报错 / 超时 / 返回非法形态一律原列表透传(歌单不会因脚本 bug 消失)。
---@alias mineral.CuratePlaylistsFn fun(lists: mineral.PlaylistBrief[]): mineral.PlaylistBrief[]
