# Roadmap

这里是一些目前预期, 以后会想做的

---

## 1. Client / Server 架构

mineral-task 与 mineral-audio 设计上已是纯进程内库,长远要装到一个常驻的 `mineral-server`,TUI / CLI 都成 client。关掉 TUI 不会杀掉音频; 一台机器同时跑 TUI 和 CLI,二者看到的是同一份播放状态。

## 2. AI Agent(可选,默认关)

虽然塞进去一个AI看起来有点违背了`简洁`,`音乐为中心`的出发点(笑), 不过感觉有一个agnet帮忙管理或推荐音乐听起来有点酷

所以预期会是， 需要手动启用的, 你不动的话， 仍然会是简洁纯粹的播放器

通过 LLM 管理 / 推荐用户的音乐:扫已 like、播放历史、跳过率、活跃时段,主动推 daily mix、整理散乱歌单、按情绪 / 场景生成播放列表、对话式查找之类.

模型自己配APIkey

## 3. Lua 配置系统

参考 yazi / nvim 的玩法,用 mlua 之类的嵌 Lua VM,把主题、键位、快捷脚本、AI prompt 模板、第三方扩展统一交给 Lua。

收益:配置文件是真正的代码,不是 toml 里写半残 DSL;用户可以写 `on_song_finish(fn)` 这种 hook;主题不再是改源码,而是 lua 表。社区分发主题 / 键位包跟 nvim 那套生态对齐。

## 4. 本地音乐 + 本地状态持久化

文件系统扫描成完整 channel:`*.mp3 / .flac / .m4a / .ncm` 等加密容器解码,扫 ID3 / Vorbis comments / lofty 元数据,按用户配的根目录注册成「本地歌单」,跟云端 channel 平铺合并。

同期上**持久化基建**(sqlite 或 sled):本地播放次数、跳过率、love 时间戳、最近播放、自定义标签 —— 全部跨 session 累计,不再每次启动都从零。这块是 AI agent(#2)能讲出有意义的话的前提 —— 没数据,LLM 推荐就是瞎猜。

## 5. 多 channel 生态

「多源融合」是 mineral 的核心卖点,但只有一个云端 channel 远不够。`MusicChannel` trait 已经抽好搜索 / 详情 / 播放 URL / 歌词 / 用户数据 —— 长远来看要扩张并维护其他Channel， 并拍平处理 ,每加一个不污染数据模型。

Cookie / token 长远归 #1 的 server 集中持有,而不是每个 client 各存一份。

## 6. 插件系统

跟 #3 同根但目标不一样:Lua 配置改的是自己,插件系统让别人扩 mineral。第三方通过 Lua 加载新 channel、新 visualization、新键位包、自定义命令,不需要改 mineral 源码。yazi / nvim 的 plugin 生态就是这个意思 —— 一个能自我演化的项目。

## 7. 多平台分发

让用户用包管理器一行装上,而不是 `cargo install` + 自己拉源码:

- **crates.io**:`cargo install mineral`,Rust 用户开箱。
- **AUR**:`mineral-bin`(预编译) + `mineral-git`(滚动)。
- **Homebrew tap**:macOS 用户主流入口。
- **GitHub Releases**:预编译二进制,Linux x86_64 / aarch64 + macOS Intel / Apple Silicon + Windows 至少各一份。

随之要把 release 流程跑通(版本号 bump、tag、changelog 自动汇总、CI 出 artifact)。
