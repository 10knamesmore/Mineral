# shelf source 实现 handoff

日期:2026-07-12/13。状态:**M1–M8 完成、功能端到端可用、1445 tests 全绿、未 commit**;剩 M5 organize(Lua 覆盖)。
分支:`worktree-shelf-source-design`,已 rebase 到 `dev`(`3040787`),工作区未提交。

> **给接手 agent**:本文记录「为什么这么做」与「做了什么」。设计契约在
> [`2026-07-12-shelf-source-design.md`](./2026-07-12-shelf-source-design.md)(同目录),那是**权威设计**;
> 本文是**实现日志 + 决策理由 + 现状 + 下一步**。你应能据此审每一行、质疑每个决策、乃至推倒重来。
> 每条决策我都标了 **[理由]** 和(若有)**[否决的替代]**——不同意就从那里推翻。

---

## 0. 怎么用这份文档

- 想**审代码**:看 §6 的逐文件地图 + §5 的坑(每个坑对应一处代码里的非显然逻辑)。
- 想**质疑决策**:看 §3,每条有理由 + 替代方案,直接反驳。
- 想**推倒重来**:看 §1 业务目标 + §2 设计模型,忽略我的实现从头设计;§7 是我没做的部分(还没锁死,最适合重新设计)。
- 想**验证现状**:`cargo t`(全仓 release nextest,应 1445 全绿);§8 有验证命令。

---

## 1. 业务目标(为什么做 shelf)

Mineral 是多源终端音乐播放器(ratatui + daemon/client)。已有网络源:netease、bilibili;聚合源:mineral(跨源收藏)。**shelf 是「本地/自管音频收藏」源**——把用户自己攒的音频文件(本地盘 / NAS / 未来 WebDAV)接入统一的 `MusicChannel`,和网络源一视同仁地搜索、浏览、播放。

**核心不变量**(定名 `shelf` 的由来):承诺的是「**我的收藏、我声明怎么组织**」,**不承诺物理位置**。所以不叫 `local`(NAS/远端也算)、不叫 `file`(未来流式)。经历 local→file→vault→shelf 的讨论,user 拍 `shelf`。

**与网络 channel 的分界判据**:协议是否**持有音乐实体模型**(曲目/专辑/艺人)。持有(Subsonic/Jellyfin/网易云)= 独立 channel;不持有(fs/WebDAV/SFTP/S3/网盘,文件名搜索不算)= shelf 的一种 backend。

---

## 2. 设计模型(权威在 spec 文档,这里是骨架)

1. **ID = uuid + 映射表**:`SongId` 用随机 uuid 当裸值,persist 表 `uuid ↔ path+(size,mtime)`。**不用路径当 id**——路径当 id 则 rename/移动 = 新歌、收藏与统计断链(MPD 二十年的痛)。rename 靠 (size,mtime) 匹配复用 uuid。Album/Artist 用**确定性派生 ID**(规范化分组键),不需映射表。
2. **一个「本地歌单」= 一个目录**:只收目录第一层的音频文件(spec §4 默认约定)。artist/album 一律信 tag,**绝不猜目录名**(MPD/beets 先例)。
3. **两层 Lua 配置**:数据旋钮(roots/scan)+ organize 映射函数(路径→playlist/album/artist)。**organize 尚未实现(M5)**,当前是默认目录分组代偿。
4. **storage backend trait**:`ShelfStorage` 抽象(fs 是 MVP 唯一实现),未来 WebDAV/SFTP/S3 只加实现。
5. **持久化 + daemon 同高度**:索引落 persist,扫描是 daemon 级 task。

---

## 3. 架构决策(逐条:是什么 / 理由 / 替代 / 谁定)

### D1. 定名 `shelf`,`SourceKind::LOCAL` 改名 `SHELF`
- **[理由]** 不承诺物理位置;顺带解掉 Lua 关键字坑(`sources.local` 点语法非法,`sources.shelf` 合法);persist 无 `local:` 落盘数据,改名零成本窗口。
- **[否决]** local(不准确)、file(未来流式)、vault(user 觉得 shelf 更好)。
- 代码:`source.rs` 常量 `SHELF`(name `"shelf"`,label `▣ shelf`)。

### D2. `PlayTarget` 删除(user 质疑后)
- 初版给 `ShelfStorage` 加了 `play_target(path) -> PlayTarget{Local,Remote}`。
- **[理由]** 与 model 的 `MediaUrl{Local(PathBuf),Remote(Url)}` **完全重复**(headers 本就在 `PlayUrl.stream_headers`)。fs 播放解析根本不用问 backend——channel 从索引 path 直接构 `PlayUrl{url:MediaUrl::Local(path)}`。
- **流式未来不动 MediaUrl**:MediaUrl 是可序列化**定位符**(用于 cover_url/source_url/avatar_url 等 metadata),字节流不是定位符、序列化不了。流式落在**引擎 open 音源接缝**(`mineral-audio/engine.rs:368` 已 `match MediaUrl{Remote→StreamDownload::from_stream(HttpStream),Local→open_local}`,引擎已会 `SourceStream`;SFTP/WebDAV 从 OpenDAL 拿 `SourceStream` 加第三条 arm 即可)。
- **结果**:`ShelfStorage` trait 收敛到 **2 方法**:`list_dir`(async)+ `open`(sync `Read+Seek`,探测/封面要随机访问)。**这是可被推翻的**——如果 reviewer 认为 backend 该显式声明播放解析,可以加回一个方法,但注意别和 MediaUrl 重复。

### D3. `mineral-probe` 作独立共享 crate(探测落点)
- 音频内容探测(格式/码率/位深/时长/tag)抽成新 crate。
- **[理由]** server 的 `resolve.rs` 已有 lofty 探测逻辑(跳 ID3 再认帧、不信扩展名),shelf 扫描也要探测,重复这套 ID3 知识是坑。`mineral-media` 是 OS 媒体集成(now playing)不是探测;shelf 不能依赖 `mineral-server`(分层:server 编排 channel)。故新建聚焦 crate。
- probe 吃 `Read + Seek` reader(非 path):server 喂 `File` / shelf 喂 backend reader,天然统一。
- **副作用**:重构了 `resolve.rs` 改用 mineral-probe,server 去掉 lofty 直接依赖(无回归)。
- **[可推翻]** 若认为不值得动 resolve.rs,可让 shelf 自己复制探测逻辑;但那样 ID3 坑就有两份。

### D4. probe 健壮化:read 失败降级为「仅格式已知」
- **[理由]** lofty 的 `guess_file_type`(detect)与 `read`(解析属性/tag)分离。detect 出格式后 read 可能因损坏/残帧失败。**旧 resolve.rs 分离 detect/read 才没丢格式**;我初版 probe 把两者耦合、read 失败就整个返回 None,把已识别的格式也丢了。修:格式来自 detect,read 失败降级返回 format-only(props/tags 空)。真实文件走完整路径,损坏文件优雅降级。
- **踩坑经过**:合成的 `id3_prefixed_mp3()` 是残帧,read 失败暴露了这个 bug(见 §5)。

### D5. shelf **是** channel(不是特殊 data source)
- user 问过「browse 都能 search 了,还需要 shelf 是 channel 吗」。
- **[理由]** `channel_for(source)`(`player.rs:274`:`channels.iter().find(|ch| ch.source() == source)`)是整个系统的路由骨架:**播放、下载、收藏聚合、Lua 桥全部按 SongId 的 namespace 路由到 channel**(player.rs:718 播放 / download.rs:497 / favorites.rs:305 聚合 / script_bridge.rs:520)。没有 SHELF channel,`channel_for(SHELF)` 返 None → shelf 歌播不了、下不了、进不了聚合、Lua 看不见。channel 是「一个 source 在系统里存在」的强制接缝,搜索只是它 ~10 个方法之一。
- **[可推翻]** 理论上可在 player/download/favorites/detail 逐处特判 SHELF 走本地路径,但那是 trait 要消灭的耦合(CLAUDE.md「TUI 只面向 trait 编程」)。

### D6. 搜索 = 内存 nucleo(channel)+ browse `/`(TUI),FTS5 不做
- user 定调:shelf 本地量小,加载进内存 fuzzy;FTS5 先不做。
- **[理由]** browse `/` 是 TUI 侧对已加载 library 的过滤(`runtime/filter.rs`,带拼音),不过 channel.search_songs。但全局搜索框(跨源)其他源都在,shelf 缺席不一致。故 channel 也实现 `search_songs`(裸 nucleo,无拼音),两个面并存。
- **caveat**:带拼音的 `FuzzyMatcher` 在 `mineral-tui/filter.rs`,channel 层够不着(层次在 TUI 下)。所以全局搜索里 shelf 的 fuzzy 比 browse `/` 弱一档(无拼音)。**若要拼音齐平**:把 filter.rs 的 matcher 抽到共享 crate,channel 和 TUI 共用——**这是干净的 follow-up**。
- **表设计因此无搜索列/索引**,只 `(mount,path)` 一个索引供 scan/reconcile。带前导通配的 `LIKE '%x%'` 用不上索引,FTS5 才是加速手段(deferred)。

### D7. 默认目录分组(organize 未实现前的代偿)
- `index/group.rs`:按 `parent_dir(path)` 分组,含音频的目录 = 一张歌单,PlaylistId = 目录绝对路径裸值(确定性、不落表可重算)。
- **[理由]** organize(M5)是这层的**可定制覆盖**;先有默认约定(spec §4)让 shelf 立即可浏览。

### D8. `SongId` 改 `define_id!` → `define_uuid!`
- **[理由]** spec §2 要 uuid 裸值,`define_id!` 无 `new_uuid`;`define_uuid!` 在 `define_id!` 基础上**额外加** `new_uuid(namespace)`(用 `mineral_macros::uuid`,故 mineral-model 不用加 uuid 依赖)。**additive**,不改既有 serde/behavior。
- 影响全工作区(SongId 到处用),但因 additive 全部编译通过。

### D9. 索引 = 单张扁平表 `shelf_file`
- migration `0004_shelf_index.sql`:uuid PK + `UNIQUE(mount,path)` + 探测快照列(format/bitrate/bit_depth/duration/title/artist/album/album_artist/track_no/genre)。
- **[理由]** `UNIQUE(mount,path)` 自带索引,兼作「按路径反查 uuid」「列一个 mount 全部文件(前导列 mount)」——省掉独立索引。分组(album/artist/playlist)**不落表**,由 group.rs 从文件事实重算(避免第二数据源漂移)。NULL 表未知,不用 0/'' 哨兵。`ShelfFileRow` 整数一律 i64(贴 sqlite),domain 换算(u64/u32/u8)在 shelf 边界做——不猜 sqlx 无符号解码支持。
- **[实现时定]** spec §8 说「建表粒度按查询需求定」;我选了单表。**organize(M5)可能需要加分类产物列或表**——见 §7。
- **迁移号踩坑**:见 §5.1(0003 碰撞)。

### D10. 索引存**绝对路径**
- **[理由]** fs 播放直接 `MediaUrl::Local(绝对 path)`,不用解析 mount 相对路径。`mount` 列作分组/dedup 键。
- **[可推翻]** 远端 backend 可能想存 mount 相对路径 + mount 标识 backend;届时 path 语义要重审。

### D11. config:`sources.shelf` 用 `#[config_section]`(非网络源)
- **[理由]** shelf 无 timeout/proxy,像 MineralSection 用 `#[config_section]`(= derive Clone/Debug/Deserialize/Getters + deny_unknown_fields + non_exhaustive),不走 `#[source_section]`。字段 color/roots/scan{exclude,max_depth,follow_symlinks}。organize 函数字段留 M5(函数不能落型进 struct,要像 curate_playlists 那样提取进 registry)。

### D12. daemon 扫描 = 启动一次(ServerConfig 快照)
- `PlayerCore::spawn_shelf_scan()` 在 `Player::spawn`(daemon 启动)调一次,**非每 client connect**(refresh_initial_loads 会每连一次,扫盘太重)。
- **[理由]** 参数走 `ServerConfig` 快照(同 favorites backfill),spawn 时从配置取。
- **[已知限制]** 改 roots 需**重启 daemon**——server 目前不重建 ServerConfig、无配置变更再扫。spec §8 想要热重载再扫,是 follow-up(需 events.rs 配置变更处理挂钩 + 重建 shelf_scan 参数)。**[可推翻]** 若认为热重载重要,应设计 config-change → 重读 roots → re-scan 的通路。

---

## 4. 实现里程碑(做了什么 / 文件 / 测试)

每个里程碑独立 clippy + 全量 nextest 验证后才进下一个。

### M1 身份 + model 地基(全绿)
- `SourceKind::LOCAL` → `SHELF`(18 文件)。用 ast-grep 结构化改写 `SourceKind::LOCAL`,但它对嵌套实参漏改,perl 收尾(见 §5.4)。**三处字符串编码身份陷阱**必须逐个手改(rename 触不到):脚本测试 id `"local:abc"`、curate fixture 键、`from_name("local")`。
- `Song.track_no: Option<u32>` + `Song.tags: Vec<String>`(genre 并入),`#[builder(default)]`+`#[serde(default)]` 保旧快照容缺。
- 更新 2 个 Debug 快照(bilibili/netease convert)+ 3 个 TUI label 快照(□ local → ▣ shelf,同宽 7 列)。
- 文件:`source.rs`、`song.rs`、`ids.rs`(M6 才改 define_uuid)、+ 各处 rename。

### M2 backend 抽象(全绿)—— 新 crate `mineral-channel-shelf`
- `storage/backend.rs`:`ShelfStorage` trait(`list_dir` async / `open` sync→`Box<dyn ShelfReader>`)+ `Entry`/`EntryKind` + `ShelfReader`(blanket impl over Read+Seek+Send)。
- `storage/fs.rs`:`FsStorage`(std::fs,`list_dir` 走 spawn_blocking,`open`=File::open)。
- **PlayTarget 后来删了**(D2)。

### M3 探测 + 扫描(全绿)
- 新 crate `mineral-probe`:`probe<R:Read+Seek> → ProbedAudio{format,bitrate_kbps,bit_depth,duration_ms,tags:ProbedTags{title,artist,album,album_artist,track_no,genre}}`。lofty `Accessor`(title/artist/album/genre/track)+ `ItemKey::AlbumArtist` + `TaggedFileExt::primary_tag`。健壮化见 D4。
- 重构 `resolve.rs` 改用 mineral-probe,server 去 lofty 依赖。
- shelf `scan/`:`walk.rs` 队列式(非递归,避免 async 递归 boxing)遍历 ShelfStorage,`ScanOptions{max_depth,exclude:Vec<Regex>}`,`is_audio_ext` 预筛 → `probe_file`(spawn_blocking)。产 `Vec<ScannedDir{path,files:Vec<ScannedFile{path,size,mtime,probed}>}>`。`result.rs` 是产出类型。

### M4 persist 索引(全绿)
- migration `0004_shelf_index.sql`(D9)。
- `db/shelf.rs`:`ShelfStore`(`upsert`/`find_uuid_by_path`/`list_mount`/`list_all`/`get`/`delete`/`update_location`=rename 复用 uuid)+ `ShelfFileRow`。`ServerStore::shelf()` 入口。降级句柄全 no-op/空。

### M6 channel 读侧(全绿)
- `index/row.rs`:`scanned_to_row`(domain→i64 换算)、`row_to_song`(i64→domain + 派生 ArtistId(normalize 艺名)/AlbumId(normalize `albumartist\u1f album`,albumartist 回落 artist))、`filename_stem`(title 兜底)。**注**:derive-getters 返 `&Option<T>`,Copy 内层要 `(*getter())` 再取 Option 方法。
- `index/reconcile.rs`:调和(path 命中更新 / gone+new 按 (size,mtime) 配对复用 uuid / 未配对发新 uuid / 消失删行;非 UTF-8 path 跳过)。
- `channel.rs` `ShelfChannel`:source=SHELF、caps(searchable=[Song]/playlist_edit=false/artist_sections=[Albums])、songs_detail(点查)、song_urls(快照直构 PlayUrl,24bit 无损→Hires)、search_songs(内存 nucleo `Pattern::score`+`Utf32Str::new`+分页)。
- `index/group.rs`(M6 Phase2):默认目录分组 + my_playlists/playlist_detail。

### M7 config + 注册(全绿)
- `schema/sources.rs`:`ShelfSection`+`ScanSection`,挂进 `SourcesConfig` + `source_colors` 加 ("shelf",color)。`schema/mod.rs` 导出。`default.lua` 加 `sources.shelf`(color #6b8e9e,roots={},scan.exclude={"^\\."},max_depth=8)。
- `loader/pipeline.rs`:`normalize_shelf_arrays` 给 `roots`/`scan.exclude` 挂 array metatable(见 §5.2 空 Lua 表坑)。
- `main.rs`:build_channels **恒注册** ShelfChannel(mineral crate 加 shelf 依赖)。
- 连带修 3 处 TUI tint 测试(shelf 有配色后不再退 subtext,见 §5.3)。`defaults_snapshot` 更新。

### M8 daemon 扫描 task(全绿)—— shelf 端到端可用
- `index/ingest.rs`:`scan_and_index(storage,store,roots,max_depth,exclude)`——遍历 roots(各为 mount),**root 先探可达性**(`list_dir` 成功才 reconcile,不可达跳过不清库=NAS 约束1),坏 exclude regex warn+跳过。
- server:`config.rs` 加 `shelf_roots`/`shelf_max_depth`/`shelf_exclude`(from_config 提取)。`shelf_scan.rs` `ShelfScanParams` 存进 `Inner`。`PlayerCore::spawn_shelf_scan()`(读 roots→scan_and_index→submit_my_playlists(SHELF))在 `Player::spawn` 启动时调一次。server 加 `mineral-channel-shelf` 依赖(唯一 server→具体 channel 耦合,spec §8 认可)。`server_config_defaults` 快照更新。

---

## 5. 坑与教训(每个对应代码里的非显然逻辑)

### 5.1 worktree 错基于 main,0003 migration 碰撞 —— 最严重
- worktree 被 EnterWorktree 默认从 `origin/main` 切,应基于 `dev`(集成分支)。dev 的 commit 57ed382(波形 seekbar)顺带加了 `0003_song_envelope.sql`,我在 main-based worktree 又建 `0003_shelf_index.sql` **撞版本号**。
- **双重失误**:①base 选错;②查 dev 多什么用了 `git diff --stat main..dev | tail -30`,persist 路径排 tui 前面被 tail **截掉**,误判「无 persist 前置」。**查「有没有动某子系统」绝不能用 tail 截断**。
- 修:migration renumber `0003→0004`;WIP 提交→`git rebase dev`(5 文件重叠但改动区不同,3-way 自动合无冲突)→`git reset --mixed dev` 恢复未提交。rebase 后一处 dev 引入的 `SourceKind::LOCAL`(playback.rs:394)要改 SHELF。

### 5.2 空 Lua 表 `roots={}` 当 map 非 sequence
- `Vec<String>` 落型报「invalid type: map, expected a sequence」→ daemon 起不来。空 Lua 表 serde 默认序列化成 map。
- 修(同 `tui.copy.templates` 先例):`pipeline.rs` `normalize_shelf_arrays` 给 `roots`/`scan.exclude` 挂 `lua.array_metatable()`。

### 5.3 shelf 加配色让 3 处 TUI tint 测试过时
- library/queue/theme sidebar 测试断言「local/shelf 未配色退 subtext」。shelf 现有配色 #6b8e9e → 染上自己的色。改成断言其配色 / theme 测试换未配置插件源(`from_name("unconfigured_plugin")`)测兜底。

### 5.4 ast-grep 对嵌套实参漏改
- `ast-grep -p 'SourceKind::LOCAL' --rewrite ...` 对 `SongId::new(SourceKind::LOCAL, ...)` 这种嵌套实参漏改(tree-sitter 节点类型差异),多趟不收敛。perl 收尾(已验证 dedicated 工具无法干净完成,例外正当)。

### 5.5 nextest fail-fast 误判「通过」
- 一个测试挂会取消后续(如「380/1325 tests run」)。别把「没跑到」当「通过」。诊断 probe 时踩过——以为 resolve 测试过了,其实被 fail-fast 取消。

### 5.6 probe read 失败丢格式(D4 的踩坑经过)
- 合成 `id3_prefixed_mp3()` 是残帧,lofty read 失败。初版 probe read 失败返 None → 丢了 detect 出的格式。修见 D4。

---

## 6. 逐文件地图(审代码入口)

**新增 crate `mineral-probe`**(`crates/mineral-probe/`):
- `probe.rs` — `probe()` + `ProbedAudio`/`ProbedTags` + `file_type_to_format`/`is_audio_ext`。核心是 lofty 封装 + D4 健壮化。

**新增 crate `mineral-channel-shelf`**(`crates/mineral-channel/shelf/src/`):
- `storage/{backend,fs}.rs` — ShelfStorage trait + FsStorage(D2 后 2 方法)。
- `scan/{walk,result}.rs` — 队列式遍历 + 产出类型。
- `index/row.rs` — ScannedFile↔ShelfFileRow↔Song 换算 + 派生 ID。**审重点**:normalize/派生 ID 的确定性、i64↔domain 换算边界。
- `index/reconcile.rs` — 调和算法。**审重点**:rename 匹配(size,mtime)、删除逻辑、非 UTF-8 跳过。
- `index/group.rs` — 默认目录分组。
- `index/ingest.rs` — `scan_and_index`。**审重点**:可达性探针防清库。
- `channel.rs` — MusicChannel impl。**审重点**:song_urls 的 PlayUrl 构造、fuzzy_search/paginate。

**新增**:`persist/migrations/0004_shelf_index.sql`、`persist/src/db/shelf.rs`(ShelfStore)、`server/src/shelf_scan.rs`(扫描 task)。

**修改**(47 文件,大多是 LOCAL→SHELF rename;实质改动):
- `model/{source,song,ids}.rs` — 身份 + 字段 + define_uuid。
- `server/resolve.rs` — 改用 mineral-probe(去 lofty)。
- `server/{config,player,lib}.rs` + `player/tests.rs` — 扫描 task wiring。
- `config/schema/sources.rs` + `loader/pipeline.rs` + `default.lua` — config 段 + 空数组修复。
- `main.rs` — build_shelf 注册。
- TUI:`library.rs`/`queue.rs`/`theme.rs` — tint 测试修。
- 快照:6 个 `.snap`(2 Debug + 3 TUI label + defaults + server_config)。

---

## 7. 下一步:M5 organize(未做,最适合重新设计)

**目标**(spec §3):两层 Lua 配置的映射函数。`organize(dir)` 纯函数,输入带 tag,返回 nil(跳过)/ string(歌单名)/ table(结构化声明:playlist/album/artist/albumartist/year/cover/tags/sort)。合成:**声明 > tag > None**。让 `~/Music/惘闻/八匹马/*.flac` 可声明成 artist=惘闻/album=八匹马(纯路径,不读元数据)。

**架构(spec §8 定,但存储粒度「实现时定」——最适合 reviewer 重新拍)**:
1. **config 提取**:`sources.shelf.organize`(Lua function)不能落型进 struct,要像 `curate_playlists` 那样在 `pipeline.rs` 提取进 named registry(键 `SHELF_ORGANIZE_FN`)。参照 `extract_playlist_transforms`。
2. **脚本 IPC**:VM 在独立单线程,daemon 经 `ScriptSender` 请求/响应调(参照 `library.rs:283 sender.curate_playlists(...).await → CurateOutcome`)。要加 `sender.organize(dir_brief, timeout) → OrganizeOutcome`:mineral-script 侧构 `dir` Lua 表(path/name/parent/depth/root/files{path,name,tags})→ 调 registry fn → 解析 nil/string/table 三返回。**这是最重、最该仔细的部分**(Lua marshalling + 三形态解析 + fail-open)。
3. **daemon 集成**:`spawn_shelf_scan` 在 scan 与 reconcile 之间逐 dir 调 organize、应用覆盖。**注意**:`scan_and_index` 现在把 scan+reconcile 打包;要插 organize(需 VM,在 shelf crate 外)得拆开——scan(shelf)→ organize(daemon/script)→ reconcile(shelf)。
4. **存储 + 查询**(**未锁死,reviewer 定**):
   - 选项 A:organize 产物(playlist 名 / album/artist 覆盖)存进 `shelf_file` 新列或分组表,`my_playlists` 按它分组。「改 organize 只重跑分类零 IO」= 重跑 organize + UPDATE 列(不重探)。
   - 选项 B:scan 时应用 organize 覆盖到 row(简单,但改 organize 需重扫)。
   - 我倾向 A(符合 spec「分类与扫描解耦」),但 A 要 migration + daemon 分类 pass + my_playlists 改。
5. 规模估计 ~300+ 行跨 5 crate(config/script/server/shelf/persist)+ migration + 多轮全量。

**为什么没在这轮做**:它是自成一体的 Lua-IPC 子系统,值得干净实现 + 逐 crate 测,不宜在长 session 尾赶。默认目录分组(§D7)已代偿其基本功能。

**其他 follow-up**(spec §11 非目标 + 本实现留的口子):
- 配置热重载再扫(§D12 限制)。
- 全局搜索拼音齐平(§D6:抽 filter.rs matcher 到共享 crate)。
- album_detail/artist_detail 下钻(现 default NotSupported)。
- 远端 backend(WebDAV/SFTP,§D2 接缝已立)。
- netease `no`(track_no)字段:spec 说要 `cargo apitest` 实证 wire 有没有;**我没做**(现 track_no 只 bilibili 填)。

---

## 8. 验证 / 现状

- **测试**:`cargo t`(= `nextest run --workspace`,release)→ 应 **1445 passed, 0 failed**。`cargo td`(doctest)另跑。
- **lint**:`cargo clippy --workspace --all-targets -- -D warnings`(严格,禁 unwrap/panic/as/wildcard,pub 必带文档,单函数≤300 行,单文件≤800 行非 test)。
- **快照**:改 model/config 影响 6 个 `.snap`,已 review + 接受(diff 只含预期新增)。改快照必 review,禁 `INSTA_UPDATE=always` 盲接受。
- **git**:未提交,worktree 在 dev 上。落 dev 前:spec/handoff 文档不 commit(`docs/superpowers/specs/` 惯例);`git add -p` 只挂 shelf hunk。
- **手动冒烟**(reviewer 可试):`sources.shelf.roots = {"/path/to/music"}` 写进 config.lua → 重启 daemon → shelf 侧栏出现「每个含音频目录一张歌单」。

---

## 9. 关键不变量(推倒重来也要守)

1. **不用哨兵值**:缺失一律 `Option`/NULL,不用 0/''/-1。
2. **平铺合并契约**:`mineral-model` 无 source-specific 字段;shelf 数据映射到统一类型。
3. **source ≠ channel**:source=身份(SourceKind,烙进 ID namespace);channel=连接器(MusicChannel impl)。
4. **uuid 稳定跨 rename**:路径当 id 是禁忌。
5. **组织语义信 tag 不猜目录**(除非 organize 显式声明)。
6. **MediaUrl 是定位符**:字节流不进它(D2)。
7. **测试不豁免 lints**;快照带中文 description;probe 内容探测不信扩展名(跳 ID3)。
