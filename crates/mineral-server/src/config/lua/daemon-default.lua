local KB = 1024
local MB = KB * 1024
local GB = MB * 1024

---@type mineral.DaemonConfig
return {
  audio = {
    volume = 100,
    backend = "auto",
    playback_quality = "exhigh",
    engine_tick_ms = 20,
    prefetch_bytes = 256 * KB,
    tap_capacity = 8192,
    envelope = {
      points = 200,
      block_ms = 100,
      window_ms = 400,
      shelf = {
        f0_hz = 1681.974450955533,
        gain_db = 3.999843853973347,
        q = 0.7071752369554196,
        band_exponent = 0.4996667741545416,
      },
      highpass = {
        f0_hz = 38.13547087602444,
        q = 0.5003270373238773,
      },
    },
  },
  cache = {
    audio_capacity = 10 * GB,
  },
  download = {
    quality = "lossless",
    dir = nil,
    max_concurrent = 3,
    tagging = true,
    tagging_workers = 4,
  },
  sources = {
    ["local"] = {
      roots = {},
    },
    mineral = {
      backfill = {
        chunk_size = 40,
        max_concurrent = 3,
      },
    },
    netease = {
      album_cache = {
        ttl_days = 30,
        ttl_jitter_days = 7,
      },
      requests = {
        album_detail_requests_per_second = 1,
        retry_delays_ms = { 500, 1000, 1500 },
      },
      playlist_fetch = {
        batch_size = 500,
        max_concurrent = 3,
      },
      timeout_secs = 100,
      proxy = false,
      max_connections = 0,
    },
    bilibili = {
      timeout_secs = 100,
      proxy = false,
      max_connections = 0,
    },
  },
  queue = {
    transforms = {},
  },
  gapless_prefetch_ms = 10000,
  prev_restart_threshold_ms = 3000,
  player_tick_ms = 20,
  session_save_secs = 15,
  heartbeat_secs = 180,
  report_interval_ms = 200,
  seek_threshold_ms = 1000,
  download_speed_tick_ms = 150,
  channel_workers_per = 8,
  script = {
    watchdog_instruction_interval = 2000,
    watchdog_soft_wall_ms = 100,
    watchdog_hard_wall_ms = 1000,
    hook_timeout_ms = 2000,
  },
  stats = {
    level = "full",
    collect = {},
    search_queries = "raw",
    exclude_sources = {},
    session_gap_minutes = 30,
    retention_days = false,
    report = {
      min_listen_secs = 30,
      top_limit = 10,
    },
  },
}
