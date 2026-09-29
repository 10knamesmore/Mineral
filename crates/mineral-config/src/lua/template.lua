---@type mineral.Config
-- Mineral 用户配置

-- 示例:
-- mineral.on("track_started", function(args)
--   mineral.ui.card({
--     title = "Now Play",
--     ttl_secs = 6,
--     body = {
--       {
--         { (" "):rep(3) },
--         { args.song.title, fg = "accent", bold = true, italic = true, align = "center" },
--         { (" "):rep(3) },
--       },
--       { { args.song.album, align = "center" } },
--       { { args.song.artists[1], align = "center" } },
--     },
--   })
-- end)

return {
  -- 示例:把初始音量调到 80
  -- audio = { volume = 80 },

  -- 示例:换主强调色 + 重映射暂停键
  -- tui = {
  --   theme = { accent = "#f38ba8" },
  --   keys = { play_pause = "x" },
  -- },

  -- 示例:歌单列表呈现
  -- sources = {
  --   bilibili = {
  --     curate_playlists = function(lists)
  --       local keep = {}
  --       for _, p in ipairs(lists) do
  --         if p.track_count > 0 and p.name:match("^音乐") then
  --           keep[#keep + 1] = p
  --         end
  --       end
  --       return keep
  --     end,
  --   },
  -- },
}
