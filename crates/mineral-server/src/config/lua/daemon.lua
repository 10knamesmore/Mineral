---@type mineral.DaemonConfig
return {
  -- audio = { volume = 80 },
  --- 注册音乐操作;api 由 daemon 传入,不需要 require。
  ---@param api mineral.DaemonApi
  setup = function(api)
    -- api.hook("before_stream", function(ctx)
    --   if ctx.song.duration_ms and ctx.song.duration_ms < 10000 then
    --     return { skip = "short track" }
    --   end
    -- end)
  end,
  -- queue = {
  --   transforms = {
  --     { name = "Reverse", transform = function(songs, ctx)
  --       local reversed = {}
  --       for i = #songs, 1, -1 do reversed[#reversed + 1] = songs[i] end
  --       return reversed
  --     end },
  --   },
  -- },
  -- sources = {
  --   bilibili = {
  --     curate_playlists = function(lists)
  --       local keep = {}
  --       for _, playlist in ipairs(lists) do
  --         if playlist.track_count > 0 then keep[#keep + 1] = playlist end
  --       end
  --       return keep
  --     end,
  --   },
  -- },
}
