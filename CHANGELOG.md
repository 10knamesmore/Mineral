# Changelog
## [0.5.5] — 2026-07-23

### Features

- 队列结构编辑全栈 + 面板增强 + Lua 变换 ([`0a09eef`](https://github.com/10knamesmore/Mineral/commit/0a09eef3d649a27c316f4d90c1f5b314be2167c7))

- Queue 浮层 / 模糊过滤——顶栏输入 + 命中高亮 + 按分排序 ([`dcd760c`](https://github.com/10knamesmore/Mineral/commit/dcd760c8601c0dbeb6ea0d0bdcaf006ce0936d49))

- 全屏频谱高度响应式——占屏高百分比 + 上下限钳制 ([`fb0ecb1`](https://github.com/10knamesmore/Mineral/commit/fb0ecb1532e3481b6f8ac46c86e2692450d6de5c))

- Top artists 改口味口径——听了谁的歌,而非从谁的页面起播 ([`539d357`](https://github.com/10knamesmore/Mineral/commit/539d357808cbc13ee5f6c9d08de1deaa18329bb2))

- Top albums 改口味口径 + CLI 文本输出表格化 ([`39a609e`](https://github.com/10knamesmore/Mineral/commit/39a609e1f76542edbec12fb4e947bc593609e2fd))

- 全屏歌词焦点行 seek 游标 + Enter 跳到该行 ([`b075c94`](https://github.com/10knamesmore/Mineral/commit/b075c94b106e5dfb5f99b9f06f177cb6e1a3d490))

- 整屏形变封面飞行层——跨端 fade 飞行根治端点瞬换 ([`0656f79`](https://github.com/10knamesmore/Mineral/commit/0656f7969f65ac80f270dfa0a5b961965378440c))

- 全屏氛围背景滞后跟随几何 + theme.background 消 toggle 跳变 ([`689db86`](https://github.com/10knamesmore/Mineral/commit/689db862ca95ba06b15f68462de53632b4d75912))

- Selected 面板身份分层——标题/副信息/meta 三行居中,在播=标题行高亮 ([`0ce01fd`](https://github.com/10knamesmore/Mineral/commit/0ce01fd57bf0eaa492e4def065c008f27ea49ceb))

- 滚动期 cache-hit 封面 halfblock 兜底——debounce 只挡高清编码 ([`395bf7e`](https://github.com/10knamesmore/Mineral/commit/395bf7e36c0d8180e772062e2ca5a4ada8fd5698))

### Bug Fixes

- 盲文频谱暗点保底改单向平滑,根除随背景漂移的移动黑斑 ([`4a1de68`](https://github.com/10knamesmore/Mineral/commit/4a1de6879f4a644a9a0fb19d61e9916b3a0feca3))

- Play-next 补偿 remove 左移,紧随当前曲不再空一格 ([`4dc7cbe`](https://github.com/10knamesmore/Mineral/commit/4dc7cbe4fe473cf86bc79d3c6a6a63a571e69a04))

- Playlists→tracks 首进封面消 hash 闪——悬停期暖入口曲第0首 ([`a36dc78`](https://github.com/10knamesmore/Mineral/commit/a36dc7840ecb45f4c779183b08b97375321e7c61))

- 下钻 sweep 过场消背景露洞——copy_col 把离屏 Reset 底视作透明 ([`4504fe5`](https://github.com/10knamesmore/Mineral/commit/4504fe5a2f1d9a8777ec4cf525af12b78a41ab3e))
## [0.5.4] — 2026-07-18

### Features

- 进度条振幅波形 seekbar ([`57ed382`](https://github.com/10knamesmore/Mineral/commit/57ed3825c1a2c216b975949790d36b83886df378))

- Spectrum.style 渲染风格枚举 ([`7ce5f94`](https://github.com/10knamesmore/Mineral/commit/7ce5f94c29014b3cc2bc1a9368e28056ff7eb755))

- 封面取色驱动动态 accent——effective theme 合成,切歌全局渐变 ([`89d7654`](https://github.com/10knamesmore/Mineral/commit/89d765462fbad010bcafc90adf46016da2517c9f))

- 行为埋点系统——28 事件表 + plays 全 provenance + CLI 报告 ([`4f44b67`](https://github.com/10knamesmore/Mineral/commit/4f44b671ef85df61a4a2db999efe8eaba0ef5944))

- Config.override 表对象形——Config 偏表拍平标量叶子,批量原子覆盖 ([`9957398`](https://github.com/10knamesmore/Mineral/commit/9957398c7fa7e23311f36b19f0c79a8c13d383ec))

- Search 选中行高亮随焦点环渐变——detail 失焦补 pale 对称 ([`4d234c3`](https://github.com/10knamesmore/Mineral/commit/4d234c3f2b7290b2e094250f78aa590e06fc28f6))

- LuaCATS type stub 从 Rust schema 宏生成 ([`1b0d1e0`](https://github.com/10knamesmore/Mineral/commit/1b0d1e068473a896465b8d75afa16621786d8847))

- 容器 o 菜单加 Play all next——整列按序插播,倒序保序 + 空队列退化 ([`edee60b`](https://github.com/10knamesmore/Mineral/commit/edee60b4722cfd9890d035004bd884a28133a669))

- 报表自足化——songs 维表 + context_name 快照,删跨库回查 ([`773b80a`](https://github.com/10knamesmore/Mineral/commit/773b80a2bc80700c3c0ce08b6e7bab9cebc8baa3))

- Album/artist 复制菜单加 Copy URL + Lua 模板扩展到 album/artist ([`46143ae`](https://github.com/10knamesmore/Mineral/commit/46143aedb3237ff0a9aa816152f334ad132eaada))

- Alias入 deep search+ 两搜索面统一括注样式与命中高亮 ([`21e9a4a`](https://github.com/10knamesmore/Mineral/commit/21e9a4a563ef0014bacdc53695608445494f4054))

- 多 client 接入——TaskEvent 订阅化 + busy 门退役 ([`f1d7954`](https://github.com/10knamesmore/Mineral/commit/f1d7954a3d0d7b618ce7f7e438f349d9e8e38c49))

- Client 连接生命周期埋点——client_connections 表 ([`f1bc2b9`](https://github.com/10knamesmore/Mineral/commit/f1bc2b99de5c76536638a2f57b5775762133f748))

- Queue 浮层宽档加 album 列 ([`34f87fd`](https://github.com/10knamesmore/Mineral/commit/34f87fd5dd908465d0ac16239ff424a08782b4e8))

- 全屏 ambient 调色板渐变背景 + 切歌封面 halfblock 转场 ([`a1eaa68`](https://github.com/10knamesmore/Mineral/commit/a1eaa684a2c703200e01d09537e1b5866130d4b3))

- Kitty transmit 流式化 + 多终端适配(ambient 挖洞 / 图协议强制与降级) ([`baf9519`](https://github.com/10knamesmore/Mineral/commit/baf9519071990ba9536df0cfd3e3e25ec6a2271b))

- Ambient / cover_transition 默认值按真机观感调参 ([`27d89d9`](https://github.com/10knamesmore/Mineral/commit/27d89d99919e9ea637ee4905178e61c3d42b8c9f))

- 次级文本对实际背景 alpha 化 + 盲文频谱亮度保底 ([`7bb5868`](https://github.com/10knamesmore/Mineral/commit/7bb58686999fa56daaa1a25582bd8b6d4a9eef00))

- 氛围背景响度跳动——PCM 响度包络驱动浓度/色斑/亮端/暗角四目标调制 ([`76a2a42`](https://github.com/10knamesmore/Mineral/commit/76a2a42a3f6e07eabb965d4ca0e874757a0d5857))

### Bug Fixes

- Lua 类型标注补 ANSI 色值写法 ([`4d136ef`](https://github.com/10knamesmore/Mineral/commit/4d136efe1792ccc34b0b94d53e65d491885efd7f))

- 配置推送成功路径彻底静默——清掉 flash 行为的残留 ([`72a1109`](https://github.com/10knamesmore/Mineral/commit/72a11090ce84d2220ee6dade3e621198d8f33eeb))

- 序号列宽随长度自适应 + 实际硬上限 9999 ([`a24634f`](https://github.com/10knamesmore/Mineral/commit/a24634fe09ca689691450844a2e144d0b05430a3))

- Search 封面接入滚动防抖——SearchPage 自持选中时间戳 ([`c25bf25`](https://github.com/10knamesmore/Mineral/commit/c25bf254c7d5bae64c8ee0d6e2200f8db0f6528a))

- Daemon e2e 的 action_output 对 busy 拒绝加重试 ([`8f9df04`](https://github.com/10knamesmore/Mineral/commit/8f9df04a773ebbc52199b1d8399cfcb0ff8f58f1))

- 全屏封面免重编码——协议缓存 per-URL 多尺寸槽位 + 进入形变按终态尺寸预热 ([`df56b5b`](https://github.com/10knamesmore/Mineral/commit/df56b5bca3f980446f8bb7d17f172678cf7bffad))
## [0.5.3] — 2026-07-07

### Features

- 封面形变期与编码等待改用 halfblock 真图,消除全屏落定瞬间 hash 闪 ([`6c1d69d`](https://github.com/10knamesmore/Mineral/commit/6c1d69d481853b99c1b92243daca9f512312ba43))

- 滚动期封面画低清真图而非留空,kitty 编码压到停稳后 ([`08e37c9`](https://github.com/10knamesmore/Mineral/commit/08e37c95db4976941dbf0b73b9b4aa0eba85a6c3))

- Prewarm playlist entry track covers ([`6397288`](https://github.com/10knamesmore/Mineral/commit/6397288b7cbf7e3d9c1ef04e080bef58a28f4a88))

- 接入 Bilibili 音源 ([`560359e`](https://github.com/10knamesmore/Mineral/commit/560359ee31413ae81e93ab9f9e27fb7a78daede2))

- 分片流 StreamLayout 流式打开 + capture 按 Content-Length 校验入缓存 ([`3becb1a`](https://github.com/10knamesmore/Mineral/commit/3becb1a63aa3ffdb916dec1a0b26b12b254ab1e9))

- 收藏改为本地 persist 事实来源 + channel 远端镜像/导入 ([`649924c`](https://github.com/10knamesmore/Mineral/commit/649924c158d2635da82fca1cab6db3e7fa8a51c3))

- 显式 has_more 翻页信号 + web url 位置占位模板 ([`f7329cd`](https://github.com/10knamesmore/Mineral/commit/f7329cde5a8fe286da147f285a300d43770a0f6f))

- Source/kind 下拉白名单配置 + source 下拉按源徽标色着色 ([`3579419`](https://github.com/10knamesmore/Mineral/commit/35794198332927276dbbec21d727d733925c6434))

- Curate_playlists 两级歌单策展 + server 聚合快照 ([`c2bb673`](https://github.com/10knamesmore/Mineral/commit/c2bb6731f04539ac79971a60ab63733cd924d932))

- Before_stream 拦截管道 + 无版权曲跨源补救全链 ([`e089fbd`](https://github.com/10knamesmore/Mineral/commit/e089fbd0265dca6fb1e879c99bf441d4fdb036cb))

- 播放态驱动的终端窗口标题 ([`d1db8ff`](https://github.com/10knamesmore/Mineral/commit/d1db8ff082a42a416abd1bc25a5b92c11c3a5ba6))

- Mineral 源聚合全源收藏 + 缺 meta 后台节流回填 ([`cab21f3`](https://github.com/10knamesmore/Mineral/commit/cab21f3163c71e0b215cf27666ebd8940395a331))

- 封面原图/协议双字节预算 LRU + prefetch 按预算收窄 ([`4e292cf`](https://github.com/10knamesmore/Mineral/commit/4e292cff1b45eb76d2f5b85fb8b5e37187a3fdff))

- 视频源建模为 BV→Album,消除 P1 投影 + artist 分区能力声明 ([`9b07c0a`](https://github.com/10knamesmore/Mineral/commit/9b07c0a43ce4e29ed690d0db291088c5f1f6b322))

- Album.track_count 提为 Option<u64>,未知画 `-` + 详情回填 ([`8d0a2e5`](https://github.com/10knamesmore/Mineral/commit/8d0a2e5a56cbef307e2adf79d7ed81cf0c88e340))

- 全局 ? 键位 cheatsheet 浮层 ([`5a5d8fd`](https://github.com/10knamesmore/Mineral/commit/5a5d8fdc7dec353f518f00933b33fd34a1b67d95))

- Schema 版本化迁移(sqlx::migrate)+ mineral cache reset 删库重建 ([`eb41562`](https://github.com/10knamesmore/Mineral/commit/eb41562dff0bd95e1e2da0bf89d52c3daac25ce1))

- Duration_ms 全链去 0 哨兵改 Option,修 bilibili gapless 预排从不触发 ([`15722dd`](https://github.com/10knamesmore/Mineral/commit/15722dd61dadc2bd649ae815d3c33f241600fd84))

- Follower/bitrate/size/format 去哨兵改 Option,fmt 段不再显 0kbps 撒谎 ([`63221be`](https://github.com/10knamesmore/Mineral/commit/63221be6da45dac89938c1fe6a09288e23f8a0c8))

- DB 约束交给库执行 + tui.db 禁 JSON(track_pos 专表 + client 迁移) ([`f37e6be`](https://github.com/10knamesmore/Mineral/commit/f37e6be22df4319295e43b8cd07913473f0681cd))

- 封面下载走图床服务端缩放(仅传输层,缓存 key/模型仍原始 URL) ([`45c1a3a`](https://github.com/10knamesmore/Mineral/commit/45c1a3a73c7525ba72322d0c28184347d3c6523b))

- Cover.download_workers 默认 4→12 ([`15e4327`](https://github.com/10knamesmore/Mineral/commit/15e432790003473b792e9627912754d1528c506c))

- Song 译名字段 translation→alias + 全链渲染/持久化/搜索,修网易别名来源 ([`63567f3`](https://github.com/10knamesmore/Mineral/commit/63567f3f0d7d6dd05d5195c57545fa3d8dc2643c))

- Tracks 面板左上角加 source 徽标 ([`71e74d5`](https://github.com/10knamesmore/Mineral/commit/71e74d5c3883769a624c07c93bb3420b0d9b7799))

- 溢出标题 marquee 滚动(loop/bounce/off)+ 边缘 fade ([`7b71d7e`](https://github.com/10knamesmore/Mineral/commit/7b71d7e606a21fe882eed92f1761dda7eb58e666))

- Not playing 全屏封面改旋转唱片纹待机动画 ([`1f4306b`](https://github.com/10knamesmore/Mineral/commit/1f4306bab0a65f391dc780abe26bfbe64ae52105))

- Mineral 聚合歌单封面改成员真封面拼贴 ([`13eae14`](https://github.com/10knamesmore/Mineral/commit/13eae14192c6813bfd215cfdf6062b5dac4181cc))

- 颜色 token 可引用终端 ANSI 槽 / 终端默认(跟随终端配色) ([`7324ad5`](https://github.com/10knamesmore/Mineral/commit/7324ad5a04fd64a29e667c8c231979b4316f6d33))

- Daemon 托管配置——overlay 合成推送 + client apply_config 单入口 ([`dd671bd`](https://github.com/10knamesmore/Mineral/commit/dd671bd6b36b6a3a696f1a007726eb1de3a8bbc8))

### Bug Fixes

- Detail 下钻 sweep 对齐切区/左栏——读 view_sweep + ease-in-out ([`0102dc8`](https://github.com/10knamesmore/Mineral/commit/0102dc883e72c2e54c9e64e560aec1068cbbef01))

- 队列含重复曲时按下标推进,根治两首交替曲死循环 ([`3e9e339`](https://github.com/10knamesmore/Mineral/commit/3e9e339870840318fcf4a9675d729df4c6cd92a6))

- Queue 浮层按下标标 ▶,重复曲不再两行一起高亮 ([`d3ef58b`](https://github.com/10knamesmore/Mineral/commit/d3ef58b7365a6b3b65ff6e9c51f532452165e58c))

- 终端字号变化后刷新 picker 字号,修复封面尺寸错乱 ([`fb84240`](https://github.com/10knamesmore/Mineral/commit/fb84240b2635567415cf1b2115d3934c350b4730))

- 窗口标题 OSC 消毒 / 图标名还原 / 热重载 + 渲染收尾 ([`633f793`](https://github.com/10knamesmore/Mineral/commit/633f79315fccb14bc17c7e33e77f8bf2c082f5e2))

- Now_playing selected strip 别名跟随整行高亮,不再固定 dim ([`a2ff4ff`](https://github.com/10knamesmore/Mineral/commit/a2ff4ff07f1522999394211967a90cf6331ef63c))

- 移除切 source 致 kind 落首项的 FlashKind 提示 ([`6496292`](https://github.com/10knamesmore/Mineral/commit/6496292a3095a7ac643530be9beb8f1ae31cc446))

- Duration_ms 跟进 Option<u64> 化,修 mock channel 编译失败 ([`0e47970`](https://github.com/10knamesmore/Mineral/commit/0e479701dbaf7696f71fd945bd451ecf0496001e))
## [0.5.2] — 2026-06-21

### Features

- 终端失焦标识——顶栏渐变变灰 + not focused 徽标 ([`8fec297`](https://github.com/10knamesmore/Mineral/commit/8fec2974be3e6e39c4000b2e1c60fa6fb0683277))

- MusicChannel 增加歌单写操作与 caps 能力声明 ([`fce1533`](https://github.com/10knamesmore/Mineral/commit/fce15336d66335c4f61470a5f99ba4f42db8ec86))

- MockChannel 歌单写操作内存实现 ([`7114200`](https://github.com/10knamesmore/Mineral/commit/7114200d2a8344c43f22e4e4277d4344558955bc))

- 歌单写操作 + 歌手端点全套落地 ([`5241ffa`](https://github.com/10knamesmore/Mineral/commit/5241ffa9eaeb98214ddbbf9836c6912be9ba5584))

- 搜索任务管线 + 歌单写执行器 + 队列插播 ([`a3a11d4`](https://github.com/10knamesmore/Mineral/commit/a3a11d4ff07930b671d35e482b13657453ba4f9e))

- PopMenu 锚定菜单 + o/y 上下文操作/复制 + copy.templates Lua 模板全链 ([`127cd65`](https://github.com/10knamesmore/Mineral/commit/127cd65edb7b0ff2aa188b1265efcd531c98b84d))

- PopMenu 形变进场 + 横向对齐(关键字/比例) + 收回缓动 + l 确认 ([`1c5fdee`](https://github.com/10knamesmore/Mineral/commit/1c5fdee988402986833a12a2d65a8b1a7b8dc98d))

- Search 布局态 + morph 形变(compute_search 三端点 / OpenSearchView / z/s 互斥) ([`60141ce`](https://github.com/10knamesmore/Mineral/commit/60141ceb2b1f97adc82ab2c8a146e16d58b7eafd))

- Search 布局态交互重做 + 结果列结构化 + channel_search 提顶层 ([`b0964fc`](https://github.com/10knamesmore/Mineral/commit/b0964fc5c83e15d78f2645dac1fdffbce16f798e))

- Detail 端点 noun 化 + 实体详情栈下钻交互 ([`ba93384`](https://github.com/10knamesmore/Mineral/commit/ba9338474f6f8c40d273692754b50732b3e30a5d))

- Browse 行为页化 → impl Page for BrowsePage + BrowseEffect ([`9d7c4d2`](https://github.com/10knamesmore/Mineral/commit/9d7c4d28844761970394baf79447df593d4a9d1f))

- Tui border use rounded ([`d9cf98e`](https://github.com/10knamesmore/Mineral/commit/d9cf98eadbd8079dc8cebc114970c4ccebbda674))

- Search 输入框接管顶行 — 去 status bar + 顶栏 morph 收掉 ([`0435212`](https://github.com/10knamesmore/Mineral/commit/04352122076cb1ab53d5da8f838572b140155e39))

- Detail 顶栏 title 运行时按栈顶实体 breadcrumb 生成 ([`f0114d4`](https://github.com/10knamesmore/Mineral/commit/f0114d4687b04929cb1631bb1cb5669341894cca))

- Search detail 曲目表对齐 browse + album 结果加 tracks 列 ([`f8f9069`](https://github.com/10knamesmore/Mineral/commit/f8f906943bc3cf3eb65f6f287edb0947ccda7f8d))

- Artist detail Albums 表加列 + Top Songs↔Albums 切换动画 ([`4da91b6`](https://github.com/10knamesmore/Mineral/commit/4da91b6396918ebdf9d7ff44a8d4f966f595faf7))

- Detail 头部简介多行渲染 + C-d/u/b/f 滚动 ([`f63a281`](https://github.com/10knamesmore/Mineral/commit/f63a281015eea14e10c8b75d9d14cfa725c9e79e))

- Search result/detail 面板左下角加位置标 ([`b34e599`](https://github.com/10knamesmore/Mineral/commit/b34e599bc5abc0e3eb3097fecf5d6f3146d73d9c))

- 搜索结果懒分页预取触发(behavior.search_prefetch_rows) ([`5672de1`](https://github.com/10knamesmore/Mineral/commit/5672de1719491a379690985b7dd696c645497d35))

- List y/o 菜单接入所有 surface(单 resolver + 组件收口) ([`8ed90ac`](https://github.com/10knamesmore/Mineral/commit/8ed90ac7e38594abeb1baf27f33a5570733ea956))

- Search/detail loading 三态 + 可配 spinner ([`685d186`](https://github.com/10knamesmore/Mineral/commit/685d1867976295086e34f0e048f822af22bf651e))

- Detail meta↔list 居中短线分隔 ([`2696811`](https://github.com/10knamesmore/Mineral/commit/2696811347a6ada5464c808a443d1020da7eeec7))

- 搜索光标改行内反色罩字符(ab[c]d)替代独立 █ 块 ([`9ec1d72`](https://github.com/10knamesmore/Mineral/commit/9ec1d724fc21c861220211b0e57df4c758428ed8))

### Bug Fixes

- 列表统一 ScrollList 组件，根治 search detail/results focus 贴边 ([`247058a`](https://github.com/10knamesmore/Mineral/commit/247058a17e61e299b4caa6a4fca737576f5ad628))

- 简介折行前压平制表符等控制字符 ([`d306504`](https://github.com/10knamesmore/Mineral/commit/d306504e01df0ff2b1c09ce772bc560fb36d4098))

- Reorder netease search kinds ([`f5855f6`](https://github.com/10knamesmore/Mineral/commit/f5855f6661b7df1ee378e632d1edbb0b73ecb257))
## [0.5.1] — 2026-06-10

### Breaking Changes

- 歌词单轨化,翻译/罗马音装配期互最近邻配对 ([`2df781d`](https://github.com/10knamesmore/Mineral/commit/2df781d3499dfe0673e77ee259dd8f04fb5dd058))

### Features

- IPC 优雅停止 — mineral stop 命令 + TUI Shift+Q ([`c91beee`](https://github.com/10knamesmore/Mineral/commit/c91beeeb8c152178d16007d87e8d947d313b824c))

- 统一歌词模型 + 全屏沉浸手动滚动 ([`ab7ecdd`](https://github.com/10knamesmore/Mineral/commit/ab7ecddacf491d954f9a8223635fc0265747cc11))

- 沉浸滚动边界 rubber-band 回弹 ([`7ef3320`](https://github.com/10knamesmore/Mineral/commit/7ef3320dac0c65678586dd05500caf420a55391a))

- 列表 nvim 滚动手感 + scrolloff + 平滑视口滚动 ([`ba264d5`](https://github.com/10knamesmore/Mineral/commit/ba264d54f57c8f2f2e236983c5ec33fd2710e60e))

- Playlists 深度搜索 + 命中样式/定位可配 ([`1ef25f1`](https://github.com/10knamesmore/Mineral/commit/1ef25f1ae8419ab62e01a18e2ed2b6c4c7322399))

- 歌单内光标位置记忆 + 屏上相对位置精确还原 ([`a769d63`](https://github.com/10knamesmore/Mineral/commit/a769d638475fde3cad015b9ed6fe8faefc36e999))

- 多行通知卡片 + 通用样式 span + TTL 边框倒计时 ([`7584469`](https://github.com/10knamesmore/Mineral/commit/758446924a564a1e867f4f612f975a03aeeca8f0))

- 浮层/卡片进出场内容跟动(离屏窗口搬运替代纯色空壳) ([`dd7de87`](https://github.com/10knamesmore/Mineral/commit/dd7de876ec9b234431a4356c95292859a4ddd1d1))

### Bug Fixes

- Cli_smoke 在 macOS 上 socket 路径超长误报 ([`0f5651f`](https://github.com/10knamesmore/Mineral/commit/0f5651faf887bb2eda3482ab90082df41d036f3d))

- 容忍网易 t=-1 无时间轴哨兵(JSON 负 t + 畸形 [00:00.00-1]) ([`cf59006`](https://github.com/10knamesmore/Mineral/commit/cf59006833e9e7875255c6598bbbc222bc380c7e))

- Linux MPRIS 测试补齐 LyricLine 单轨化新增字段 ([`c808640`](https://github.com/10knamesmore/Mineral/commit/c80864086db32163620db8a47b64e0fbed41e2f0))
## [0.5.0] — 2026-06-07

### 功能

- Sidebar 搜索接 fuzzy 匹配 + 拼音(全拼/首字母)过滤

- 中央 Action 枚举 + Keymap 默认表(键位可配前置,PR-A 纯增)

- Lua 用户配置 sub01 — loader + schema + config CLI

- Sub02 全量声明旋钮接线 — default.lua 单一真相源

- ADSR 时间制包络 — 快攻慢放,全配置面时长旋钮 ms 化

- Transport gapless prefetch 标记 — ⏭ 旁 ⇣ 拉取中暗/就绪亮

- 跨重启恢复 play_mode + 歌词副轨档,周期落盘加空态守卫

- 协议切 Frame 管线 — request-id 配对 + Event 交错下推 + 版本守门握手

- Mineral-script crate — Lua VM 专用线程 + watchdog + 脚本 API 面

- 脚本运行时接线 — 同 VM 加载配置 + 事件双路下发 + action 触发链

- 脚本生态数据面+触发面 — store/queue/library/timer + action ctx + error reason

- 脚本生命周期 — 热重载 + bind + nvim 键语法 + Notice 退役

- 强力位四件套 — library.search + 同步拦截 hook + spawn + bus

- Client UI 通路 — terminal 复合属性 + ui.override 旋钮覆盖

- Track_started 事件 — track_finished 的对偶

- Mineral.sys 命名空间 + Song 投影丰富 + download 事件携带音质/格式

### 修复

- 频谱过渡被打断时从可见中间色继续渐变,不再跳变

### 性能

- PlayerSync 版本门控同步替换 PlayerSnapshot 全量轮询

- 缓存存加工后产物 + 64px 取色,封面管线 CPU 大降

## [0.4.2] — 2026-06-03

全屏沉浸播放态(`z` 进出)+ gapless 无缝播放 + 频谱 / 封面观感打磨。

### 功能

- **全屏播放态**:`z` 进出,封面 / 歌词 / 频谱沉浸布局,进退场整屏形变动画;全屏歌词平滑平移 + 行间距(Apple Music 风格)。
- **gapless 无缝播放**:预排下一曲 decoder + 边界轮转,曲间无空隙(此前列为「本期不做」,现已落地)。
- **频谱**:封面取色铺到频率轴、从当前可见配色缓动到静止;低频去平板 + 峰值混合增强动态;隐藏 label;全屏下加高。
- **transport**:显示在播音频规格(format · bitrate · 采样率);进度条叠加缓冲进度。
- **网易云**:读取单曲真实累计播放次数,选中歌展示。
- 全屏 queue 浮层(与浏览布局同宽高、贴右);tracks / queue 列宽随窗口响应式两档。
- 整屏 expand / collapse 以光标真实位置为缩放锚点。
- 搜索态迁入 sidebar 标题,支持 vim 退格退出(移除独立 status_bar)。

### 修复

- 全屏切歌时封面不再不加载:封面请求与浏览选中解耦,跟随在播曲并沿播放队列预取邻近封面。
- 修全屏封面残影 / 丢失。

### 性能

- 封面 resize / base64 编码离线到 worker 线程池,切歌 / 关浮层不再卡帧;全屏稳态提前编码下一首封面,自动切歌零闪。

## [0.4.1] — 2026-05-30

修缓存 / 下载库重播不显示音频格式。

### 修复

- 本地命中(缓存 / 下载库)重播时,`PlayUrl.format` 改走 lofty `Probe`(按文件内容、跳过 ID3 标签再认底层帧)。旧实现用 `FileType::from_buffer`,一见 `ID3` 前缀即整片漏判,NetEase exhigh 等 FFmpeg 转码的 mp3 格式显示为空(FLAC 因 magic 在偏移 0 不受影响)。走 `Probe::new`(reader,无路径)而非 `Probe::open`,保住「只认内容、不信扩展名」契约。下载库里带 ID3 的 mp3 同样修复。

## [0.4.0] — 2026-05-30

本地缓存 / 下载库体系成型(文件系统为真相、sqlite 索引);macOS 系统媒体集成。

### 缓存 / 下载库

- 缓存索引迁移到 **sqlite 写穿透**,弃用 BlobCache / bincode。
- 下载库改以**文件系统为真相**,移除 `download_export` 索引——历史下载 / 换机拷库 / 手动放入的文件一律可见,不受索引漂移影响。
- 下载不再复制进缓存;缓存仅由「边播边 capture」自然形成,职责分离;补端到端测「下载 → 播放走下载库」。
- 本地优先解析:播放前按音质从高到低查缓存 / 下载库,命中则跳过整条网络取链路径(同音质优先缓存,更高音质优先下载库)。
- `mineral cache status` 子命令查看缓存占用;`clean` 展示清理效果。

### 媒体集成

- macOS 系统 Now Playing 集成:Control Center + 媒体键(配合既有 MPRIS,双平台系统媒体控制就绪)。

### TUI

- 播放栏标记播放来源(cache / download / remote);本地播放显示真实 format / bitrate。
- 统一详情视图封面高度,消除 playlist / tracks 切换时的封面跳变。

### 路径 / 平台

- 统一跨平台 XDG 目录解析,加固 socket 路径解析。

### 其他

- 默认播放音质 Lossless → Exhigh(默认 `BitRate` 亦由 Higher 改 Exhigh)。

### 测试

- 真实 TCP I/O 测试改 multi_thread runtime,消除全仓并发 flaky。

## [0.2.0] — 2026-05-24

client/server 架构落地:播放进 daemon,关 TUI 不停播;接入系统媒体服务;测试覆盖成体系。

### 架构 — client / server 分离

- 抽 `mineral-server`(audio + task + 播放上下文收成 `Server` / `ClientHandle`)与 `mineral-protocol`(IPC 协议 crate,Request/Response + length-delimited + bincode)。
- `PlayerCore` 持播放上下文(队列 / 当前歌 / 歌词 / prefetch),daemon 自治 auto-next;PCM 走 wire —— 真正「关 TUI 不杀播放」。
- TUI 走 unix socket 连 daemon;默认启动 = 优先 attach 已有 daemon,否则 **spawn 独立 daemon 进程**再 attach;保留 `--connect`(强制连)/ `--in-proc`(同进程调试);`KILL_SPAWNED_DAEMON_ON_EXIT` 旋钮决定退出时是否带走自起的 daemon(待 lua 配置接管)。
- daemon graceful shutdown(收 SIGINT/SIGTERM 清 socket),信号 handler 提前到 bind 之前消除启动竞态。
- client 断连(daemon 被单独 kill)不再僵死:检测断开 → 记日志 + 盖断连提示 modal,等按键退出;TUI 进程收 SIGTERM/INT/HUP 先记日志再走正常退出,不 silent dead。

### 媒体集成(MPRIS)

- 接入系统媒体服务 `org.mpris.MediaPlayer2`:上报当前播放、响应媒体键 / 桌面控件;`xesam:asText` 同步当前歌词(给 quickshell 等)。
- Shuffle / LoopStatus 双向同步:4-variant `PlayMode` ↔(shuffle × repeat)二维无损塌缩。
- seek 时补发 `Seeked` 信号。

### 歌词

- channel 层输出结构化歌词,消费方零解析;MPRIS / UI 共用。

### 日志 / 可观测性

- 全链路结构化埋点;错误统一 `mineral_log::chain`(完整 context 链、单行、无 ANSI / backtrace)。
- 日志改人读单行格式(本地时间 + target + `file:line` + 字段),压低 symphonia / reqwest / hyper / stream-download 等第三方噪音。
- 60s 心跳(server + client 双侧)上报内部状态;netease 反序列化走 `serde_path_to_error`,错误带字段路径。

### TUI

- `top_status` 后台任务按 `ChannelFetchKind` 拆分计数,cover loading 显真实数。
- prefetch 失败的歌单不再每帧无限重提交(request-once dedup)。
- `sidebar/playlists` 列宽改 `Constraint::Fill` 消除 ratatui 列宽求解非确定(帧间列宽闪烁)。

### 测试

- 覆盖从 ~12% 提到 145+ 测试:player 队列 / shuffle / 模式逻辑、纯逻辑函数(format / layout / color)、protocol codec round-trip、netease wire 与 LRC 解析、daemon 进程级 e2e(`CARGO_BIN_EXE`)、CLI 冒烟。
- 引入 **insta 快照**:TUI 组件用 `TestBackend` 渲染 + `assert_snapshot!`(不依赖真 pty),解析层用 `assert_debug_snapshot!`;全部带中文 description,版本号用 `filters` 归一化。展示性 fixture 用真实曲目(Mineral《EndSerenading》/ Chinese Football / MyGO!!!!!《迷跡波》)。
- CLAUDE.md 新增「测试约定」节。

## [0.1.0] — 2026-05-03

首个 alpha 版本。从老仓库重写,把核心闭环跑通。

### 架构

- workspace 拆 13 个 crate,职责按 model / channel / task / audio / spectrum / tui / cli 分层。
- `MusicChannel` trait(async)统一抽象搜索 / 详情 / 播放 URL / 歌词 / 用户数据;数据模型平铺,新加 channel 不污染。
- `mineral-task`:优先级 lane(User / Background) + 取消 + dedup,封面 / 歌单 / 歌词分别走自己的 worker。
- `mineral-paths`:XDG 标准目录(config / data / cache)解析 + 跨平台 fallback。
- `mineral-log`:`tracing` 后端 + 文件 appender,业务侧用 macro facade 调。
- 全仓 `anyhow → color-eyre`;workspace 全局 lints(unsafe / unwrap / panic / as / wildcard import 一律 deny,函数 ≤ 300 行)。
- HashMap / HashSet 全部换 `FxHashMap` / `FxHashSet`(显式名,无 alias)。
- nightly toolchain + edition 2024,`rust-toolchain.toml` 钉住。

### 音频

- rodio 0.22 + symphonia + stream-download:支持 mp3 / aac / m4a / flac 流式播放。
- seek 全链路打通,`p` 键 iTunes 行为(>3s 回开头,否则上一首)。
- auto-next + 大跨度 seek(`Shift+←/→` 30s);auto-next prefetch 提前拉下一首 SongUrl,曲终命中跳过等待。
- armed 状态机过滤过期 PlayUrlReady,修切歌时误跳。
- Shuffle 一次性洗牌、Repeat / RepeatOne 循环模式。
- cubic 音量曲线,默认 100。

### 数据源

- 一个云端 channel(加密 + cookie + 端点)接入,搜索 / 歌单 / 歌曲详情 / 播放 URL / 歌词 / liked 列表全部就绪。
- mock channel(opt-in feature),离线开发不打任何端点。

### TUI

- 双视图 sidebar:playlists / library,Table 渲染,列对齐。
- now_playing 右栏:真实封面(ratatui-image,kitty / iTerm2 / sixel / halfblock 自适配),selected 面板按 cell 像素比横向铺满,字号变化按 dims 重建,滚动期间跳过 protocol 重建(80ms 防抖,参考 yazi `image_delay`)。
- 视口 prefetch:cover / playlist tracks 按 sel ± 64 提前拉。
- queue 浮层 + 全局播放键穿透(空格 / n / p / m / 音量 / seek)。
- 频谱面板:realfft 真值 + baseline + peak hold + 余韵 trail + 弹簧物理 + 色相漂移,bar 数随窗口动态。
- 歌词:LRC 行级 + YRC 字符级 wipe(30fps 字符级渐变),Apple Music 风格 fade,中心行换 accent 色。
- transport:title / artist · album / 进度条 / 播放控制 / 音量 + 循环模式 + 真实 fmt(format · bitrate)。
- 搜索过滤(`/` 触发):playlists 按 name,library 按 name / artists / album,case-insensitive,命中子串高亮(peach + bold + underline)。
- 视图切换 / Esc 清搜索词;Library 内 search 不影响选中歌单。
- 列表辅助:`g` / `G` 跳首末、`Shift+J/K` 7 行大跳、`n / m` 位置指示、`♥` gutter(loved 标记)、`♫` 当前播放标记。
- top_status:左 mineral + 真实 version + tabs,右后台 task 计数 + 播放状态。
- panic hook 链:Tui::enter 把 restore_terminal 接进 panic hook,确保彩色报告不被 alternate screen 吞。

### CLI

- `mineral channel netease login`:终端二维码扫码登录,凭证写入 `<data_dir>/netease.json`。

### 配置 / 路径

- 配置 / 数据 / 缓存目录走 XDG(`$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` / `$XDG_CACHE_HOME`,fallback `~/.config` / `~/.local/share` / `~/.cache`)。
- 日志默认写 `<cache_dir>/mineral.log`。

### 元数据 / 文档

- README:特性 / 构建 / 运行 / 登录 / XDG 路径 / 全套快捷键 / 架构 / 开发命令。
- ROADMAP:6 条长远方向(client/server / AI agent / Lua / 本地音乐 / 多 channel / 插件)。
- CLAUDE.md:codebase 约定 + lint 政策 + 体量约束 + 容易踩的坑。
- workspace 元数据:license = MIT / repository / authors / rust-version 一处定义,所有 crate `.workspace = true` 继承。
- per-crate description 补全。

### 已知不做(本期)

- gapless playback(rodio 上游限制)
- 多源在线 search lane(本地过滤够用)
- AuthRefresh lane(cookie 过期 UI 静默)
- 歌词翻译 / 罗马音切换 UI(字段就绪,UI 缺切换入口)
- plays 列接真值(等本地持久化基建)
- LocalScan + 本地 channel + .ncm 解码(等持久化基建)
