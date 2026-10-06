## 仓库总览

Mineral 是一个多源, C-S 架构音乐播放器(tui as a client)

数据库使用结构化列和关系，禁止存储或处理 JSON。

调用`scripts/check.sh` 跑代码编写完成后的验证

测试优先使用真实进程 E2E 或 App 输入入口，验证；除非用户主动要求, 否则不要写测试

不为配色、视觉布局、动画帧或展示文案编写断言与快照。

**版本号只由 CI release workflow 更新， 禁止手改版本**

## 规范

`mineral-model` 的设计原则是"平铺合并":模型不应放 source-specific 字段。任何 channel 实现都把网络/本地原始数据先映射到 `mineral-model` 的类型,再交给上层

- 配置默认值放在lua里面,不要在 Rust 里另设默认值。
- 配置按 daemon 与 TUI 两个宿主归属：`crates/mineral-server/src/config/` 和 `crates/mineral-tui/src/config/` 各自集中根类型、全部子 schema、默认 Lua、用户模板、LuaLS 生成与加载校验；不按具体 UI 组件拆分。`mineral-config` 只提供共用求值、合并、落型、诊断与资产写出能力；`mineral-script` 拥有脚本执行、回调 registry 与共享 Lua API/实体元数据，不依赖宿主配置类型。CLI 协调两个宿主的 init/check。
- `daemon.lua` 与 `tui.lua` 按文件区分宿主,直接返回配置根表与可选的 `setup(api)`,不接受顶层 `daemon` / `tui` 包装。两者拥有独立默认值、VM、覆盖和重载;IPC 只传业务状态与能力,不传配置或源码。

* \*\*do not use guard value (`0` / `""` / `-1` / `usize::MAX` etc ), use Option::None

* use structural Error instead of anyhow/String

- 将use 写在文件最开头, 而不是 full qualified 或者其他地方, 用来便于阅读文件开头就知道是用了哪些模块
