local KB = 1024
local MB = KB * 1024
local GB = MB * 1024

---@type mineral.Config
-- Mineral 默认配置。用户 config.lua 经深合并覆盖此表(数组整体替换)。
-- 顶层只做纯计算,勿在此放副作用(多进程各 eval 一次)。
-- 字段注解(---@class / ---@field)的真相源在 lua/meta/config.lua,本表只 ---@type 引用。
return {
  tui = {
    -- 主题色板(默认 Catppuccin Mocha)。色值还可写 { ansi = "blue" }(终端 ANSI 槽,
    -- 跟随终端配色)/ { reset = true }(终端默认),写法详见 docs/configuration.md。
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
      search_hit = { -- 搜索命中字符的样式(列表高亮)
        color = "peach", -- token 名(随主题联动)或具体色值("#rrggbb" / { ansi = ... } 等)
        modifiers = { "bold", "underline", "italic" }, -- 叠加效果,数组整体替换;可选 bold/italic/underline/dim/reversed/crossed_out
      },
    },
    keys = {
      -- 方向是【动作 → 键】;值为单键或键数组(数组整体替换)。
      -- 键写法与 nvim 对齐:单字符直接写("j" / "G" / "+"),
      -- 特殊键 / 修饰用尖括号("<Space>" / "<CR>" / "<C-g>" / "<S-Left>")。
      play_pause = "<Space>",
      next = "n",
      prev = "p",
      toggle_fullscreen = "z",
      open_search = "s",
      open_queue = "<Tab>",
      quit = "q",
      open_help = "?",
      cycle_lyric = "t",
      enter_search = "/",
      activate = { "l", "<CR>" },
      back = { "h", "<Esc>", "<BS>", "<C-h>" },
      drill_into = "<C-l>",
      cycle_detail_section = { "[", "]" },
      cycle_mode = "m",
      volume_up = { "+", "=" },
      volume_down = { "-", "_" },
      seek_forward = "<Right>",
      seek_backward = "<Left>",
      seek_forward_big = "<S-Right>",
      seek_backward_big = "<S-Left>",
      move_down = { "j", "<Down>" },
      move_up = { "k", "<Up>" },
      move_down_big = "J",
      move_up_big = "K",
      move_first = "g",
      move_last = "G",
      love = "f",
      download = "d",
      dismiss_notice = "x",
      open_action_menu = "o",
      open_copy_menu = "y",
      scroll_line_down = "<C-d>",
      scroll_line_up = "<C-u>",
      scroll_page_down = "<C-f>",
      scroll_page_up = "<C-b>",
      -- 脚本动作绑定:`mineral.action` 注册名 → 键(默认无)。
      -- 例:script = { ["my.skip_short"] = "X" }
      script = {},
    },
    behavior = {
      volume_step = 5, -- 单次音量增减,百分点
      seek_step_secs = 5, -- 单次 seek 步长,秒
      seek_big_step_secs = 30, -- 大步 seek(Shift),秒
      list_jump_rows = 7, -- 列表大步跳行数(J/K)
      scrolloff = 3, -- 光标与列表视口上下边缘的最小行距(nvim 'scrolloff');0 = 贴边才滚
      line_scroll_rows = 1, -- 单行档滚动(<C-d>/<C-u>)一次滚的行数;列表与全屏歌词共用
      page_scroll_rows = 15, -- 翻页档滚动(<C-f>/<C-b>)一次滚的行数
      search_prefetch_rows = 8, -- 搜索结果懒分页预取半径:光标距已加载末行 ≤ 此行数且未榨干时,自动拉下一页
      kill_spawned_daemon_on_exit = true, -- 退出 TUI 连带关掉自己拉起的 daemon;false = 续命后台播放
      remember_track_pos = "session", -- 歌单内光标位置记忆:"off" 不记 | "session" 本次运行内 | "persist" 跨重启落盘
    },
    -- 频谱面板。时长旋钮均为毫秒,按 animation.frame_tick_ms 折算成拍,与帧率解耦。
    -- 条高动态 = 效果器 ADSR 包络:attack 起音 / decay 衰减(余韵) / release 释音,
    -- sustain 即 FFT 实时值本身。
    spectrum = {
      fft_size = 4096, -- FFT 窗,样本数,2 的幂;大 = 低频细节多但延迟高。外键:audio.tap_capacity 须 ≥ 2 × 此值
      f_min = 20, -- 频率轴下界,Hz
      f_max = 20000, -- 频率轴上界,Hz;超奈奎斯特自动取一半
      log_axis_blend = 0.92, -- 频率轴对数化 0-1:1 = 纯对数;略小于 1 收掉低频「宽平顶」
      db_floor = -65.0, -- dB 标定下界,低于此条高 0;抬高 = 砍安静细节整体变矮
      db_ceil = -6.0, -- dB 标定上界,高于此满高;与 db_floor 共同决定显示动态范围
      peak_mix = 0.5, -- 频带统计峰值占比 0-1:0 = 纯均值(平),1 = 纯峰值(躁)
      show_peak_cap = true, -- 条顶 ▔ 浮标
      show_trail = true, -- peak 与条之间的余韵渐隐
      hue_rotate = true, -- 无封面色时色相缓慢漂移
      spring_peak = true, -- peak 弹簧物理(过冲 + 回弹);false = 直接吸附
      baseline_min = 3, -- 条最小高,1/8 字符格(满高 64);静默时面板不死寂
      attack_ms = 50, -- 起音:上升 90% 到位时长;越小越贴鼓点
      decay_ms = 100, -- 衰减:播放中余韵滑落时长;动画感来自这里
      release_ms = 200, -- 释音:暂停后落向 baseline 的时长
      peak_hold_ms = 192, -- 新 peak 原位悬停时长
      peak_fall_ms = 512, -- peak 从满高落到 0 的满程时长
      hue_cycle_ms = 30 * 1000, -- 色相转满一圈(360°)的时长
      cover_fade_ms = 6 * 1000, -- 封面取色就绪后的配色过渡时长
      cover_vshift_permille = 200, -- 封面色场顶端沿色带的纵向偏移,‰;拉开条底/条顶层次
      spring_stiffness = 0.35, -- 弹簧刚度;无量纲系数,与帧率耦合
      spring_damping = 0.45, -- 弹簧阻尼;越小回弹越多,越大越稳
    },
    -- 进度条波形:transport 进度条化身全曲振幅波形。包络只对本地/已缓存曲目可算,
    -- 未就绪自动回落普通进度条。「全屏才展开」等场景化开关走脚本 override,见文档 recipe。
    waveform = {
      enabled = false, -- 进度条化身振幅波形;包络未就绪自动回落普通进度条
      cover_color = true, -- 已播放段吃封面取色;false 用主题 accent
    },
    -- 封面管线:抓取 → 解码缩放 → 磁盘缓存 → k-means 取色喂频谱。
    cover = {
      http_timeout_secs = 30, -- 单张封面下载超时,秒
      max_dim = 384, -- 解码后等比缩放到的最大边,px;终端显示足够,大了费内存
      jpeg_quality = 100, -- 重编码质量 1-100;仅 storage = "resized" 时生效
      storage = "resized", -- "raw" | "resized";resized = 缓存命中只解 ≤max_dim 小图,CPU 大降
      debounce_ms = 80, -- 列表滚动停稳多久才渲染真图;期间显示程序化色块占位
      download_workers = 12, -- 封面下载并发 worker 数
      encode_workers = 2, -- 终端图片协议编码并发 worker 数
      cache = { -- 缓存预算(LRU,满了自动驱逐);磁盘项改小不立刻删文件,下次写入时驱逐
        disk = 4 * GB, -- 磁盘缓存上限,字节
        image = 128 * MB, -- 解码原图 RAM 预算,字节;越界逐出最久未显示者
        protocol = 64 * MB, -- 已编码终端协议(序列+源图副本)RAM 预算,字节;越界逐出最久未渲染者
      },
      kmeans = { -- 取色(频谱封面配色);取出的色不满意再动
        sample_dim = 64, -- 取色采样边长:64² ≈ 4 千像素,够聚类、极省 CPU
        swatches = 6, -- 重点色上限(聚类 k);色多层次细、色少更整体
        seed = 0x5EEDC0DE, -- 聚类种子,必须固定,否则同一封面每次取色不同
        max_iter = 20, -- 最大迭代次数
        converge = 5.0, -- 收敛阈值,Lab 空间
        l_min = 8.0, -- 丢弃近黑像素的明度下限(Lab L),避免黑背景霸占色板
        l_max = 92.0, -- 丢弃近白像素的明度上限
        chroma_min = 8.0, -- 丢弃近灰像素的彩度下限
        min_valid_pixels_pct = 5, -- 过滤后有效像素低于此(%)改用全部像素,保黑白封面有色
      },
    },
    -- 预取:提前抓即将看到的数据,用网络/内存开销换流畅度。
    prefetch = {
      radius = 64, -- 列表选中行上下各预取条数(封面 + 歌单曲目)
      playback_cover_radius = 3, -- 沿播放队列给在播曲前后各预取几张封面
      play_count_debounce_ms = 500, -- 选中停留超过此毫秒才查远端播放次数,防翻列表打满 API
      prewarm_ahead = 1, -- 全屏稳态提前编码后几首封面,消自动切歌的占位闪
    },
    -- 搜索:本地过滤(`/`)与 channel 远程搜索是两套互不相关的旋钮,分挂 deep / channel 子表。
    search = {
      -- 本地过滤搜索(`/`)。
      deep = {
        enabled = true, -- Playlists 视图搜索词穿透到歌单内歌曲;进搜索态时后台补拉未缓存歌单的曲目
        -- 字段级命中分折扣 0~1,0 = 该字段不参与;歌单最终分 = max(歌单名分, 歌单内最佳歌曲分),
        -- 单曲分 = max(name × 歌名分, artist × 艺人分, album × 专辑分)
        weights = {
          name = 0.6, -- 歌名
          artist = 0.5, -- 艺人(多艺人取最高)
          album = 0.4, -- 专辑
        },
        locate_on_enter = true, -- 深度命中行 Enter 进歌单后光标直接落到命中歌;false = 仍从头看
      },
      -- channel 搜索两个下拉的白名单:列出即暴露、顺序即下拉顺序,未列出的隐藏。
      -- source 名开放(插件源可写),没加载的名字静默跳过;空列表 = 防呆回退全量。
      channel = {
        sources = { "netease", "bilibili" },
        -- 封闭集合 song/album/artist/playlist/user,与各 source 可搜集合求交(保此处顺序)
        kinds = { "song", "album", "artist", "playlist", "user" },
      },
    },
    lyrics = {
      fullscreen_line_gap = 1, -- 全屏歌词行间空行数;0 = 紧排但滚动变瞬跳
      compact_line_gap = 0, -- 非全屏紧凑态歌词行间空行数
      scroll_ms = 280, -- 切行整列平移 + 高亮淡入的过渡时长
      reattach_ms = 4000, -- 有时间戳歌手动滚走后多久空闲自动回到跟随;无时间戳歌不回
      overshoot_damping = 1, -- 滚到头再滚,画面多滑出(超出行数 ÷ 此值)再弹回;越大弹得越轻
      overshoot_max_permille = 6 * 1000, -- 单次过冲上限,行的千分比(x * 1000 = x 行);0 = 关闭回弹
    },
    -- 动画。时长均为毫秒(按 frame_tick_ms 折算成拍,至少一拍);0 ≈ 一帧到位。
    animation = {
      frame_tick_ms = 16, -- 主循环帧间隔;16 ≈ 60fps,越小越流畅越费 CPU,是所有 *_ms 折算的分母
      transition_ms = 288, -- 启动扩大 / 退出收缩整屏转场
      sweep_ms = 288, -- 侧栏 歌单↔曲目 切换扫入
      list_scroll_ms = 280, -- 列表视口滚动平移(<C-d> 族与 scrolloff 触发的滚动)
      fullscreen_ms = 288, -- 全屏进退场形变
      popup_anim_ms = 288, -- 浮层(队列 / 确认框)弹出收起
      toast_anim_ms = 288, -- 顶栏通知横向展开收起
      focus_fade_ms = 288, -- 终端失焦/聚焦时顶栏变灰的淡入淡出
      search_focus_morph_ms = 240, -- 搜索焦点高亮边框滑动(search_focus_transition=slide 时)
      vinyl_rev_ms = 60000, -- 待机(无在播曲)唱片纹封面高光旋转一圈
      view_sweep = "push", -- "push" | "cover":侧栏切换是新旧一起平移还是从右盖上
      menu_reveal = "morph", -- "morph" | "directional":弹出菜单从锚点行形变而来 还是 贴边方向性揭开
      search_focus_transition = "slide", -- "slide" | "instant":搜索焦点高亮边框切换 从旧面板滑到新面板 还是 瞬移直切
      spinner_frames = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏" }, -- loading 旋转占位帧(逐帧循环);空 {} = 只留文案不画字形
      -- 溢出标题滚动(选中行 / 播放栏长歌名)。
      marquee = {
        mode = "bounce", -- "loop" 循环滚动 | "bounce" 来回往返 | "off" 关闭(维持静态截断)
        step_ms = 60, -- 每前进 1 列的毫秒;越小滚越快(步进越密越显丝滑,终端无亚像素只能整格跳)
        pause_ms = 100, -- 起步 / 选中切换后的停顿毫秒(先读到开头再开滚)
        fade_ms = 300, -- 滚动窗口边缘 fade 的渐入毫秒(选中后缓缓变暗,不突变);0 = 关闭边缘 fade
        fade_cols = 5, -- 边缘 fade 的空间宽度(列):窗口两侧各这么多列内逐级变暗
        -- 各滚动方式独有参数放各自子表。
        loop = {
          gap = "  ✦  ", -- 循环拼接处的分隔串;"" = 首尾直接相接
        },
        bounce = {
          edge_pause_ms = 800, -- 到达两端后的停顿毫秒(读完首/尾再折返);0 = 直接折返
        },
      },
    },
    toast = {
      flash_ttl_secs = 4, -- 一次性通知(下载完成 / 配置告警等)停留秒数
    },
    -- 复制菜单(y)的自定义模板,追加在内置项(title/artist/.../URL)之后;数组整体替换。
    -- 每项 { key?, label, template, context? }:template 是函数,收实体表返回剪贴板文本,
    -- 在 daemon 脚本运行时执行(超时/报错只 toast 不复制)。
    -- context = "song"(默认,收 mineral.Song)| "playlist"(收 mineral.Playlist,含 songs)。
    -- key 与内置项同字母时顶掉内置的快捷位;省略 = 仅 j/k + Enter 可达。
    -- 例:{ key = "f", label = "Copy full", template = function(s)
    --       return s.title .. " - " .. table.concat(s.artists, ", ")
    --     end },
    copy = {
      templates = {},
    },
    -- 窗口标题:把当前播放态实时写进终端任务栏 / tab 标题。
    -- 四态标题:template(播/暂共用)/ idle / disconnected 各一套 segment 数组,顺序即输出顺序。
    -- segment: { icon = true } | { field = "...", prefix?, suffix?, format? } | { text = "..." }
    --   field: title | artist | album | position | duration | source | lyric
    --   format(仅 position/duration): "clock"(默认 mm:ss)| "seconds" | { pattern = "{m}:{ss}" }
    -- icons 四态图标可覆盖;idle/disconnected 缺省 { {icon=true}, {text="Mineral"} }。
    -- 动态标题(轮换 / spinner / 自适应)走脚本 mineral.ui.window_title(text),见 docs/scripting.md。
    window_title = {
      enabled = true,
      icons = { playing = "⏸", paused = "▶", idle = "■", disconnected = "⚠" },
      template = {
        { icon = true },
        { field = "title" },
        { field = "artist", prefix = " — " },
      },
      idle = { { icon = true }, { text = "Mineral" } },
      disconnected = { { icon = true }, { text = "Mineral" } },
    },
    -- 布局,单位是终端字符格:宽 = 列数,高 = 行数。
    layout = {
      min_full_width = 80, -- 宽不足此列数退紧凑布局(无歌词/频谱面板)
      min_full_height = 24, -- 高不足此行数退紧凑布局
      fs_left_pct = 44, -- 全屏左栏(封面+transport)占宽 %,余下归歌词
      fs_spectrum_height = 14, -- 全屏底部频谱通栏高,行
      fs_transport_height = 8, -- 全屏 transport 条高,行;内容 6 + 边框 2
      dock_w_pct = 36, -- 停靠浮层(播放队列)占屏宽 %
      menu_align = "right", -- 弹出菜单相对锚点行的横向对齐:"left"|"center"|"right",或 0.0~1.0 数字精确指定比例(0 贴左 / 0.5 居中 / 1 贴右)
    },
  },
  -- 以下顶层段 = daemon/共享核心
  audio = {
    volume = 100, -- 启动初始音量 % 0-100;运行期音量不落盘,每次启动回到此值
    backend = "auto", -- "auto" | "null":auto 打不开声卡自动降级无声空跑;null 强制无声
    playback_quality = "exhigh", -- standard | higher | exhigh | lossless | hires
    engine_tick_ms = 20, -- 引擎主循环节拍;影响 seek/停止响应延迟,不建议动
    prefetch_bytes = 256 * KB, -- 流式起播前预拉字节;大 = 起播慢但 seek 命中缓冲概率高
    tap_capacity = 8192, -- 频谱 PCM 环形缓冲,样本数。须 ≥ 2 × tui.spectrum.fft_size,否则 UI 卡帧丢样本出毛刺
  },
  -- 缓存容量(LRU,满了自动驱逐;改小不立刻删文件,下次写入时驱逐)。封面缓存预算在 tui.cover.cache。
  cache = {
    audio_capacity = 10 * GB, -- 音频本体磁盘缓存上限,字节
  },
  -- 下载(永久导出,不受缓存容量约束)。
  download = {
    quality = "lossless", -- standard | higher | exhigh | lossless | hires;与播放音质独立
    dir = nil, -- 下载导出目录,绝对路径;缺省走默认(~/Music/mineral)
  },
  sources = {
    mineral = {
      color = "#2a6511", -- EndSerenading 封面绿(聚合收藏歌单的源徽标)
      -- 后台补 meta 节流:sync 导入的红心先只有 id,后台逐源(按各歌来源走各自 channel)拉 songs_detail 补全,聚合面渐进填满
      backfill = {
        chunk_size = 40, -- 每次 songs_detail 调用处理多少 id(= 聚合面刷新粒度 + 限住单次调用时长;非"请求数",请求怎么发是 channel 内部的事)
        max_concurrent = 3, -- 并行几个 songs_detail 调用(并发上限即节流;不管单次调用内部是一个还是多个请求,最多 N 个在飞)
      },
    },
    netease = {
      timeout_secs = 100, -- 单次 API 请求超时,秒
      proxy = false, -- false = 禁用;字符串 = 代理 URL(如 "socks5://127.0.0.1:1080")
      max_connections = 0, -- 到源的最大并发连接,0 = 不限
      color = "#9D2928", -- 网易云品牌红
    },
    bilibili = {
      timeout_secs = 100, -- 单次 API 请求超时,秒
      proxy = false, -- false = 禁用;字符串 = 代理 URL(如 "socks5://127.0.0.1:1080")
      max_connections = 0, -- 到源的最大并发连接,0 = 不限
      color = "#FF8cB0", -- B站品牌粉
    },
  },
  -- daemon 后端节拍。多为内部时序参数,默认值经过调校,没有明确诉求不要动。
  daemon = {
    gapless_prefetch_ms = 10000, -- 距曲尾多少毫秒开始预排下一曲(无缝窗口);太小可能退化出间隙
    prev_restart_threshold_ms = 3000, -- prev 键分界:进度超过此值回曲首,否则上一首
    player_tick_ms = 20, -- 播放核心后台循环间隔;影响自动切歌/事件转发延迟
    session_save_secs = 15, -- 播放进度周期落盘节流,秒;切歌等另有即时落盘
    heartbeat_secs = 180, -- 状态心跳日志间隔,秒;daemon 与 TUI 各打一条供排查
    report_interval_ms = 200, -- 向系统媒体控件(MPRIS)上报进度的间隔
    seek_threshold_ms = 1000, -- 进度偏离线性预期超过此值判定为 seek(供 MPRIS 上报)
    download_speed_tick_ms = 150, -- 下载测速刷新节流
    channel_workers_per = 8, -- 每个音乐源的后台并发 worker;大 = 抓取快但易撞限流
  },
  -- 脚本运行时(config.lua 顶层的 mineral.* 调用在 daemon 内真实生效)。
  script = {
    watchdog_instruction_interval = 2000, -- 每多少条 Lua VM 指令查一次墙钟;小 = 灵敏但开销大
    watchdog_soft_wall_ms = 100, -- 回调超过此时长记 warn 日志,继续跑
    watchdog_hard_wall_ms = 1000, -- 回调超过此时长被中断(只杀本次调用,脚本仍存活)
    hook_timeout_ms = 2000, -- before_stream/before_download 拦截 hook 软超时;超时放行 + warn,不卡播放
    spawn_max_concurrent = 8, -- mineral.spawn 子进程并发上限,防脚本 fork 炸;0 = 不限
  },
}
