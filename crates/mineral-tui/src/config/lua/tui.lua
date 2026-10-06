---@type mineral.TuiConfig
return {
  -- theme = { accent = "#f38ba8" },
  -- keys = { play_pause = "x" },
  -- copy = {
  --   templates = {
  --     { label = "Title", template = function(song) return song.title end },
  --   },
  -- },
  ---@param api mineral.TuiApi
  setup = function(api)
    -- api.config.override({ waveform = { enabled = false } })
    -- api.ui.toast("TUI ready", { ttl_secs = 3 })
    -- api.ui.window_title("Mineral")
  end,
}
