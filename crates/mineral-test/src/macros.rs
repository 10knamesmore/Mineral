//! 共享快照断言宏。强制带中文 `description`,写进 `.snap` 头,`cargo insta review`
//! 时逐张可辨。
//!
//! 宏体展开成 `insta::…`,在**调用方 crate** 解析 —— 故用到这两个宏的 crate 必须自带
//! `insta` 的 dev-dependency(本仓现有调用方都已具备)。
//!
//! 两个宏都关掉 insta 的 `prepend_module_to_snapshot`:`.snap` 文件名只取测试函数名、
//! 不再拼模块路径(模块归属由所在 `snapshots/` 目录体现)。如此模块搬迁时快照**跟着目录
//! 走、文件名不变**。代价是唯一性约束降为「同一 `snapshots/` 目录内函数名不得重名」——
//! 即同目录的兄弟源文件之间,快照测试函数要起不同名字。

/// 带中文描述的 Display 快照断言(`insta::assert_snapshot!`)。
///
/// 用法:`mineral_test::assert_snap!("描述", terminal.backend());`
#[macro_export]
macro_rules! assert_snap {
    ($desc:expr, $value:expr $(,)?) => {{
        insta::with_settings!({ description => $desc, prepend_module_to_snapshot => false }, {
            insta::assert_snapshot!($value);
        });
    }};
}

/// 带中文描述的 Debug 快照断言(`insta::assert_debug_snapshot!`),用于结构体 / 解析结果。
///
/// 用法:`mineral_test::assert_snap_debug!("描述", parsed);`
#[macro_export]
macro_rules! assert_snap_debug {
    ($desc:expr, $value:expr $(,)?) => {{
        insta::with_settings!({ description => $desc, prepend_module_to_snapshot => false }, {
            insta::assert_debug_snapshot!($value);
        });
    }};
}
