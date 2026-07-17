//! `stats` 子命令的纯渲染:文本 / json / markdown。
//!
//! 文本([`text`])是人读默认(分节 + 条形分布);markdown([`markdown`])是年终盘点存档 /
//! 分享形态;json 由命令层直接 `serde_json` 序列化领域类型(结构化全量,无需渲染函数)。

mod markdown;
mod text;

pub use markdown::{history_md, report_md, status_md, top_md};
pub use text::{render_absent, render_history, render_report, render_status, render_top};
