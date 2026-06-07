---@type mineral.Config
-- Mineral 用户配置。只需写要覆盖的字段,其余回落默认(深合并,数组整体替换)。
-- 编辑器补全 / 类型检查依赖同目录 lua/meta 下的 stub(本文件由 `mineral config init` 生成)。
-- 完整可覆盖字段见 lua/meta/config.lua;各字段默认值见同目录 default.lua(仅参考,程序不读)。
--
-- 本文件同时是脚本:顶层 mineral.* 调用写在 **return 之前**(Lua 的 return 必须是
-- 最后一条语句),daemon 加载时真实执行;return 的表是纯配置数据,里面不放调用。
-- 脚本 API 指南见仓库 docs/scripting.md。

-- 示例(脚本层,取消注释即生效):
-- mineral.on("track_started", function(args)
--   mineral.log.info("playing: " .. args.song.title)
-- end)

return {
  -- 示例:把初始音量调到 80
  -- audio = { volume = 80 },

  -- 示例:换主强调色 + 重映射暂停键
  -- tui = {
  --   theme = { accent = "#f38ba8" },
  --   keys = { play_pause = "x" },
  -- },
}
