## 仓库总览

Mineral 是一个多源音乐播放器(tui as a client)

目前只有我一个人开发, 只要这句话还存在, mineral就属于pre release 迭代期间, **禁止考虑任何持久化兼容**, 允许破坏更新, 一切设计不应该被`向后兼容`捆住手脚

埋点sql走 migration(我个人使用使用希望), 其他的不管是sql/json文件/路径契约都是可以重建的, 如果有充足的理由证明破坏更新后是更好的设计, 直接做, 本地文件可以rm

测试运行器是 **cargo-nextest**(需 `cargo install cargo-nextest cargo-insta`);`cargo t` / `td` / `snap` 是 `.cargo/config.toml` 里的 alias。

所有测试**禁止**debug run, debug 运行花的时间比 release 编译多得多, 大头时间都在编译, **禁止**先跑部分测试再跑全量，浪费大头编译时间， 直接跑全量

**版本号只由 CI release workflow 更新， 禁止手改版本**

## 架构要点

`mineral-model` 的设计原则是"平铺合并":模型里没有 source-specific 字段。任何 channel 实现都把网络/本地原始数据先映射到 `mineral-model` 的类型,再交给上层。这意味着新增 channel 时,**不要**给 `Song` 加 source-only 字段——要么提升为通用字段,要么保留在 channel 内部 dto 里。来源由 ID namespace 表达:`Song::source()` / `Album::source()` 等从各自 `id` 派生(见下)。

ID 类型(`SongId`、`AlbumId` 等)由 `mineral_macros::define_id!` 生成

`mineral-channel-core::MusicChannel`(`async_trait`)定义 catalog、library 与 user-data 操作:搜索、详情、歌词、用户歌单和喜欢状态等能力。`mineral-playback::PlaybackProvider` 独立负责把 song identity 解析为可打开的播放资源，并封装来源鉴权与媒体 preparation。server 面向这两个 trait 组合能力；TUI 通过 `mineral_server::Client` 发请求，不直接调用来源适配器。

**术语:`channel`(适配器)≠ `source`(身份)**——`source`(`SourceKind`)是数据的**来源身份**,烙进每个 ID 的 namespace(回答"这条数据来自哪");`channel`(`MusicChannel` 实现)是 catalog / library / user-data 的**连接器 / 适配器**(回答"用哪个后端取数"),经 `channel.source()` 声明它服务哪个 source。注释与命名别把两者混用:讲 ID 归属 / `Song::source()` / `sources.<name>` 配置时用 **source(来源)**;讲搜索、详情、歌词或用户数据后端时用 **channel**;讲播放资源解析与打开时用 **playback provider**。同名异义的 `rodio` 声道 / `tokio` channel 与本词表无关,别牵连。

配置(file + session 覆盖)是 daemon 上的一份**运行时状态**:daemon mtime 轮询config.lua、合成 `merge(default, user, overlay)`、落型校验后经 `Event::ConfigChanged` 推整树给订阅 client(握手先重放一帧);**client 不看文件**(TUI 启动本地 load 一次只是自举,连上即被推送顶替)。

client 侧配置消费两条规矩:

- **现读优先**:组件直接读 `state.cfg`(Arc,换整棵即热更),**不许**构造期把配置值拷进自己字段(第二数据源)。
- 确需构造期折算 / 固化的(拍数折算、FFT 预计算、缓存预算),必须挂`App::apply_config` 单入口(就地重设:`retempo` 保动画相位、`set_budgets` 不清缓存),并配一条重载测试(仿 `mineral-tui/src/runtime/reload.rs` 的既有测试)。

- 类型标注优先 turbofish:写 `Vec::<T>::new()`、`.collect::<Vec<T>>()`,而不是左侧 `: Vec<T>`。例外:trait object 向上转型(`let x: Arc<dyn Trait> = ...`)和无法推断的 `None`(`let x: Option<T> = None`)。

* 配置的默认值都放在 `default.lua` 里面， 不要在rust里面设置default导致多重数据源

- **绝不用哨兵值(`0` / `""` / `-1` / `usize::MAX` 等)表达「未知 / 缺失 / 默认」**。「没有值」在 Rust 里只有一种正确表示:`Option<T>`(需要携带原因用 `Result` / 枚举)。
