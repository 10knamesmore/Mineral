---@meta
-- Mineral 配置类型 stub(LuaCATS)。这是 default.lua / config.lua 的注解真相源。
-- 字段集与 Rust `Config` 一一对应(守卫测试钉死);改字段两边同步。
--
-- 所有 ---@field 标为可选(`?`):从配置消费方看没有必填字段——任何字段省略都合法
-- (深合并回落 default.lua)。故 partial 的用户 config.lua 不触发 missing-fields,
-- 同时保留字段名补全与值类型检查。default.lua 的完整性由 Rust 守卫测试钉死,不靠 LSP。

---@class mineral.Config
---@field tui? mineral.TuiConfig
---@field audio? mineral.AudioConfig
---@field cache? mineral.CacheConfig
---@field download? mineral.DownloadConfig
---@field sources? mineral.SourcesConfig
---@field daemon? mineral.DaemonConfig

---@class mineral.TuiConfig
---@field theme? mineral.ThemeConfig
---@field keys? mineral.KeysConfig
---@field behavior? mineral.BehaviorConfig
---@field spectrum? mineral.SpectrumConfig
---@field cover? mineral.CoverConfig
---@field prefetch? mineral.PrefetchConfig
---@field lyrics? mineral.LyricsConfig
---@field animation? mineral.AnimationConfig
---@field toast? mineral.ToastConfig
---@field layout? mineral.LayoutConfig

---@class mineral.ThemeConfig
---@field base? string  "#rrggbb" 主背景
---@field mantle? string  "#rrggbb" 次背景
---@field crust? string  "#rrggbb" 第三背景
---@field surface0? string  "#rrggbb" 行选中 / 进度条 track
---@field surface1? string  "#rrggbb" 未聚焦边框 / 分隔线
---@field overlay? string  "#rrggbb" 暗淡文本
---@field subtext? string  "#rrggbb" 三级文本
---@field text? string  "#rrggbb" 主文本
---@field accent? string  "#rrggbb" 主强调色
---@field accent_2? string  "#rrggbb" 副强调色
---@field red? string  "#rrggbb"
---@field yellow? string  "#rrggbb"
---@field green? string  "#rrggbb"
---@field peach? string  "#rrggbb"
---@field roles? mineral.RolesConfig

---@class mineral.RolesConfig
---@field accent? string  token 名(14 token 之一)
---@field muted? string  token 名
---@field faint? string  token 名

-- 键绑定值:单键字符串(如 "space")或键数组(如 { "n", "j" },整体替换)。
---@alias mineral.KeyBinding string|string[]

---@class mineral.KeysConfig
---@field play_pause? mineral.KeyBinding  暂停 / 恢复
---@field next? mineral.KeyBinding  下一首
---@field prev? mineral.KeyBinding  上一首 / 回开头
---@field toggle_fullscreen? mineral.KeyBinding  进 / 退全屏
---@field open_queue? mineral.KeyBinding  打开播放队列
---@field quit? mineral.KeyBinding  退出确认
---@field cycle_lyric? mineral.KeyBinding  循环歌词副语言
---@field enter_search? mineral.KeyBinding  进入搜索
---@field activate? mineral.KeyBinding  进入 / 播放选中
---@field back? mineral.KeyBinding  返回 / 清搜索
---@field cycle_mode? mineral.KeyBinding  循环播放模式
---@field volume_up? mineral.KeyBinding  音量增
---@field volume_down? mineral.KeyBinding  音量减
---@field seek_forward? mineral.KeyBinding  快进
---@field seek_backward? mineral.KeyBinding  快退
---@field seek_forward_big? mineral.KeyBinding  大步快进
---@field seek_backward_big? mineral.KeyBinding  大步快退
---@field move_down? mineral.KeyBinding  下移一行
---@field move_up? mineral.KeyBinding  上移一行
---@field move_down_big? mineral.KeyBinding  大步下移
---@field move_up_big? mineral.KeyBinding  大步上移
---@field move_first? mineral.KeyBinding  跳首行
---@field move_last? mineral.KeyBinding  跳末行
---@field love? mineral.KeyBinding  切换 ♥
---@field download? mineral.KeyBinding  下载选中

---@class mineral.BehaviorConfig
---@field volume_step? integer  单次音量步长(百分点)
---@field seek_step_secs? integer  单次 seek 步长(秒)
---@field seek_big_step_secs? integer  大步 seek(秒)
---@field list_jump_rows? integer  列表大步跳行数
---@field kill_spawned_daemon_on_exit? boolean  退出时是否杀自拉起的 daemon

---@class mineral.SpectrumConfig
---@field show_peak_cap? boolean  是否显示 peak cap
---@field show_trail? boolean  是否显示 trail 余韵
---@field hue_rotate? boolean  是否色相漂移
---@field spring_peak? boolean  是否启用 peak 弹簧
---@field baseline_min? integer  条最小高度(1/8 字符单位)
---@field attack_old? integer  上升平滑旧值权重
---@field attack_new? integer  上升平滑新值权重
---@field decay_div? integer  衰减除数(指数项)
---@field decay_step? integer  衰减常数项
---@field peak_hold_ticks? integer  peak 悬停 tick
---@field peak_fall_per_tick? integer  peak 每 tick 下落单位
---@field hue_cycle_ticks? integer  色相转一圈的 tick
---@field cover_fade_ticks? integer  封面配色过渡 tick
---@field cover_vshift_permille? integer  色场纵向偏移(‰)
---@field spring_stiffness? number  弹簧刚度
---@field spring_damping? number  弹簧阻尼

---@class mineral.CoverConfig
---@field http_timeout_secs? integer  封面下载超时(秒)
---@field max_dim? integer  封面缓存最大边(px)
---@field jpeg_quality? integer  封面 JPEG 质量(0-100)
---@field storage? "raw"|"resized"  封面存储模式
---@field debounce_ms? integer  封面切换去抖(ms)
---@field download_workers? integer  封面下载并发
---@field encode_workers? integer  封面编码并发
---@field kmeans? mineral.KmeansConfig

---@class mineral.KmeansConfig
---@field swatches? integer  色板色数(聚类 k)
---@field seed? integer  kmeans 随机种子
---@field max_iter? integer  最大迭代次数
---@field converge? number  收敛阈值
---@field l_min? number  明度下限(Lab L)
---@field l_max? number  明度上限(Lab L)
---@field chroma_min? number  彩度下限
---@field min_valid_pixels_pct? integer  有效像素占比下限(%)

---@class mineral.PrefetchConfig
---@field radius? integer  通用预取半径
---@field playback_cover_radius? integer  在播曲封面预取半径
---@field play_count_debounce_ms? integer  播放计数查询去抖(ms)
---@field prewarm_ahead? integer  全屏预热前瞻首数
---@field channel_workers_per? integer  每 channel 抓取并发

---@class mineral.LyricsConfig
---@field line_gap? integer  全屏歌词行距
---@field scroll_ms? integer  歌词滚动过渡(ms)

---@class mineral.AnimationConfig
---@field frame_tick_ms? integer  主循环帧间隔(ms,≈60fps)
---@field transition_ticks? integer  整屏转场时长(tick)
---@field sweep_ticks? integer  侧栏曲目扫入时长(tick)
---@field fullscreen_ticks? integer  全屏进退时长(tick)
---@field popup_anim_ticks? integer  浮层进出时长(tick)
---@field toast_anim_ticks? integer  toast 进出时长(tick)
---@field view_sweep? "push"|"cover"  侧栏扫入风格

---@class mineral.ToastConfig
---@field flash_ttl_secs? integer  通知停留时长(秒)

---@class mineral.LayoutConfig
---@field min_full_width? integer  完整布局最小宽(列)
---@field min_full_height? integer  完整布局最小高(行)
---@field fs_left_pct? integer  全屏左栏占比(%)
---@field fs_spectrum_height? integer  全屏频谱高(行)
---@field fs_transport_height? integer  全屏 transport 高(行)
---@field dock_w_pct? integer  浮层 dock 宽占比(%)

---@class mineral.AudioConfig
---@field volume? integer  初始音量 0-100
---@field backend? "auto"|"null"  音频后端
---@field playback_quality? "standard"|"higher"|"exhigh"|"lossless"|"hires"  在线播放音质
---@field engine_tick_ms? integer  音频引擎主循环 tick(ms)
---@field prefetch_bytes? integer  流式起播预拉字节
---@field tap_capacity? integer  FFT tap 环形缓冲容量

---@class mineral.CacheConfig
---@field audio_capacity? integer  音频缓存上限(字节)
---@field cover_capacity? integer  封面缓存上限(字节)

---@class mineral.DownloadConfig
---@field quality? "standard"|"higher"|"exhigh"|"lossless"|"hires"  下载音质
---@field dir? string  下载目录(nil = 默认导出目录)

---@class mineral.SourcesConfig
---@field netease? mineral.NeteaseSection

---@class mineral.NeteaseSection
---@field timeout_secs? integer  请求超时(秒)
---@field proxy? string|false  代理 URL,或 false 禁用
---@field max_connections? integer  最大并发(0 = 不限)

---@class mineral.DaemonConfig
---@field gapless_prefetch_ms? integer  gapless 预取提前量(毫秒)
---@field prev_restart_threshold_ms? integer  prev 回开头 vs 上一首分界(ms)
---@field player_tick_ms? integer  player 主循环醒来间隔(ms)
---@field session_save_secs? integer  会话位置刷新节流(秒)
---@field heartbeat_secs? integer  client 心跳间隔(秒)
---@field report_interval_ms? integer  播放进度上报间隔(ms)
---@field seek_threshold_ms? integer  判定 seek 的跳变阈值(ms)
---@field download_speed_tick_ms? integer  下载测速刷新周期(ms)
