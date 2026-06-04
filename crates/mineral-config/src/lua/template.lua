---@type mineral.Config
-- Mineral 用户配置。只需写要覆盖的字段,其余回落默认(深合并,数组整体替换)。
-- 编辑器补全 / 类型检查依赖同目录 lua/meta 下的 stub(本文件由 `mineral config init` 生成)。
-- 完整可覆盖字段见 lua/meta/config.lua。
return {
  -- 示例:把初始音量调到 80
  -- audio = { volume = 80 },

  -- 示例:换主强调色 + 重映射暂停键
  -- tui = {
  --   theme = { accent = "#f38ba8" },
  --   keys = { play_pause = "x" },
  -- },

  -- 示例(可编程层,daemon 进程才真正执行;TUI/CLI 进程为 no-op):
  -- mineral.on("track_finished", function(song, reason)
  --   mineral.log.info("finished: " .. reason)
  -- end)
}
