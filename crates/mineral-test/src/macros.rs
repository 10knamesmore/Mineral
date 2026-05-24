//! 共享快照断言宏。强制带中文 `description`,写进 `.snap` 头,`cargo insta review`
//! 时逐张可辨。
//!
//! 宏体展开成 `insta::…`,在**调用方 crate** 解析 —— 故用到这两个宏的 crate 必须自带
//! `insta` 的 dev-dependency(本仓现有调用方都已具备)。

/// 带中文描述的 Display 快照断言(`insta::assert_snapshot!`)。
///
/// 用法:`mineral_test::assert_snap!("描述", terminal.backend());`
#[macro_export]
macro_rules! assert_snap {
    ($desc:expr, $value:expr $(,)?) => {{
        insta::with_settings!({ description => $desc }, {
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
        insta::with_settings!({ description => $desc }, {
            insta::assert_debug_snapshot!($value);
        });
    }};
}
