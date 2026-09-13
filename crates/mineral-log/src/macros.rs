//! 业务日志宏:统一添加 `mineral` target namespace,字段与格式化交给 tracing。

/// 写入 TRACE 日志;`target: "name"` 归入 `mineral::name`,省略时归入 `mineral`。
#[macro_export]
macro_rules! trace {
    ($($fields:tt)+) => {
        $crate::__log!(TRACE, $($fields)+)
    };
}

/// 写入 DEBUG 日志;`target: "name"` 归入 `mineral::name`,省略时归入 `mineral`。
#[macro_export]
macro_rules! debug {
    ($($fields:tt)+) => {
        $crate::__log!(DEBUG, $($fields)+)
    };
}

/// 写入 INFO 日志;`target: "name"` 归入 `mineral::name`,省略时归入 `mineral`。
#[macro_export]
macro_rules! info {
    ($($fields:tt)+) => {
        $crate::__log!(INFO, $($fields)+)
    };
}

/// 写入 WARN 日志;`target: "name"` 归入 `mineral::name`,省略时归入 `mineral`。
#[macro_export]
macro_rules! warn {
    ($($fields:tt)+) => {
        $crate::__log!(WARN, $($fields)+)
    };
}

/// 写入 ERROR 日志;`target: "name"` 归入 `mineral::name`,省略时归入 `mineral`。
#[macro_export]
macro_rules! error {
    ($($fields:tt)+) => {
        $crate::__log!(ERROR, $($fields)+)
    };
}

/// 日志宏的展开入口;导出仅为支持调用方 crate 中的宏展开。
#[doc(hidden)]
#[macro_export]
macro_rules! __log {
    ($level:ident, target: $target:literal, $($fields:tt)+) => {
        $crate::__tracing::event!(
            target: concat!("mineral::", $target),
            $crate::__tracing::Level::$level,
            $($fields)+
        )
    };
    ($level:ident, $($fields:tt)+) => {
        $crate::__tracing::event!(
            target: "mineral",
            $crate::__tracing::Level::$level,
            $($fields)+
        )
    };
}
