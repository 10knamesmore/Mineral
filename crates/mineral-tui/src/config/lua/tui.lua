---@type mineral.TuiConfig
return {
  -- Default colors follow the terminal; missing replies use theme_defaults.
  -- theme_defaults accepts fixed #rrggbb colors only.
  -- theme_defaults = { background = "#1e1e2e" },
  -- theme = {
  --   accent = mineral.terminal.ansi.blue,
  --   surface0 = mineral.color.mix(
  --     mineral.terminal.background,
  --     mineral.terminal.foreground,
  --     0.10
  --   ),
  -- },
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
