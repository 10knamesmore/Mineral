---@type mineral.Config
-- Mineral 默认配置。用户 config.lua 经深合并覆盖此表(数组整体替换)。
-- 顶层只做纯计算,勿在此放副作用(多进程各 eval 一次)。
-- 字段注解(---@class / ---@field)的真相源在 lua/meta/config.lua,本表只 ---@type 引用。
return {
  -- tui 段:in-repo client 专属命名空间,协议无特权、仅打包特权。
  tui = {
    theme = {
      base = "#1e1e2e",
      mantle = "#181825",
      crust = "#11111b",
      surface0 = "#313244",
      surface1 = "#45475a",
      overlay = "#6c7086",
      subtext = "#a6adc8",
      text = "#cdd6f4",
      accent = "#cba6f7",
      accent_2 = "#74c7ec",
      red = "#f38ba8",
      yellow = "#f9e2af",
      green = "#a6e3a1",
      peach = "#fab387",
      roles = { accent = "red", muted = "subtext", faint = "overlay" },
    },
    keys = {
      -- 方向是【动作 → 键】;值为单键或键数组(数组整体替换)。
      play_pause = "space",
      next = "n",
      prev = "p",
      toggle_fullscreen = "z",
      open_queue = "tab",
      quit = "q",
      cycle_lyric = "t",
      enter_search = "/",
      activate = { "l", "enter" },
      back = { "h", "esc", "backspace" },
      cycle_mode = "m",
      volume_up = { "+", "=" },
      volume_down = { "-", "_" },
      seek_forward = "Right",
      seek_backward = "Left",
      seek_forward_big = "Shift+Right",
      seek_backward_big = "Shift+Left",
      move_down = { "j", "Down" },
      move_up = { "k", "Up" },
      move_down_big = "J",
      move_up_big = "K",
      move_first = "g",
      move_last = "G",
      love = "f",
      download = "d",
    },
    behavior = {
      volume_step = 5,
      seek_step_secs = 5,
      seek_big_step_secs = 30,
      list_jump_rows = 7,
      kill_spawned_daemon_on_exit = true,
    },
    spectrum = {
      show_peak_cap = true,
      show_trail = true,
      hue_rotate = true,
      spring_peak = true,
      baseline_min = 3,
      attack_old = 7,
      attack_new = 3,
      decay_div = 4,
      decay_step = 1,
      peak_hold_ticks = 12,
      peak_fall_per_tick = 2,
      hue_cycle_ticks = 1800,
      cover_fade_ticks = 300,
      cover_vshift_permille = 200,
      spring_stiffness = 0.35,
      spring_damping = 0.45,
    },
    cover = {
      http_timeout_secs = 30,
      max_dim = 384,
      jpeg_quality = 100,
      storage = "raw", -- "raw" | "resized"
      debounce_ms = 80,
      download_workers = 4,
      encode_workers = 2,
      kmeans = {
        swatches = 6,
        seed = 0x5EEDC0DE,
        max_iter = 20,
        converge = 5.0,
        l_min = 8.0,
        l_max = 92.0,
        chroma_min = 8.0,
        min_valid_pixels_pct = 5,
      },
    },
    prefetch = {
      radius = 64,
      playback_cover_radius = 3,
      play_count_debounce_ms = 500,
      prewarm_ahead = 1,
      channel_workers_per = 8,
    },
    lyrics = {
      line_gap = 1,
      scroll_ms = 280,
    },
    animation = {
      frame_tick_ms = 16,
      transition_ticks = 18,
      sweep_ticks = 18,
      fullscreen_ticks = 18,
      popup_anim_ticks = 18,
      toast_anim_ticks = 6,
      view_sweep = "push", -- "push" | "cover"
    },
    toast = {
      flash_ttl_secs = 4,
    },
    layout = {
      min_full_width = 80,
      min_full_height = 24,
      fs_left_pct = 44,
      fs_spectrum_height = 14,
      fs_transport_height = 8,
      dock_w_pct = 36,
    },
  },
  -- 以下顶层段 = daemon/共享核心
  audio = {
    volume = 100,
    backend = "auto", -- "auto" | "null"
    playback_quality = "exhigh", -- standard | higher | exhigh | lossless | hires
    engine_tick_ms = 20,
    prefetch_bytes = 256 * 1024,
    tap_capacity = 4096,
  },
  cache = {
    audio_capacity = 10 * 1024 ^ 3, -- 字节;Lua 可编程性:不需要 "10GiB" 字符串解析
    cover_capacity = 1 * 1024 ^ 3,
  },
  download = {
    quality = "lossless", -- standard | higher | exhigh | lossless | hires
    dir = nil, -- 缺省走默认导出目录
  },
  sources = {
    netease = {
      timeout_secs = 100,
      proxy = false, -- false = 禁用;字符串 = 代理 URL
      max_connections = 0, -- 0 = 不限
    },
  },
  daemon = {
    gapless_prefetch_ms = 10000,
    prev_restart_threshold_ms = 3000,
    player_tick_ms = 20,
    session_save_secs = 15,
    heartbeat_secs = 60,
    report_interval_ms = 200,
    seek_threshold_ms = 1000,
    download_speed_tick_ms = 150,
  },
}
