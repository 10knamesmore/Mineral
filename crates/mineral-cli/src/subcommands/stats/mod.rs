//! `mineral stats` 子命令树:离线直读 stats.db(数值)+ mineral.db(名字回查),不经 daemon。
//!
//! `report` / `top` / `history` / `status` / `prune` / `reset` 六命,横切时间窗([`window`])
//! 与输出格式;报告装配([`assemble`])跑 stats.db 聚合 + 回查名,渲染([`render`])出
//! text / json / md。

mod assemble;
mod command;
mod render;
mod window;

pub use command::{StatsCommand, run};
