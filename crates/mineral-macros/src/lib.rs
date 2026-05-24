//! 提供结构化 ID newtype 的样板宏。
//!
//! - [`IdString`] 是源内的裸标识值(某个 channel 后端实际使用的 id 字符串)。
//! - [`define_id!`] 生成一个 `{ namespace, value: IdString }` 的结构化 ID newtype,
//!   namespace 决定裸值在哪个来源内解释,带 serde、常用 `Display`/构造器/访问器。
//! - [`define_uuid!`] 在 [`define_id!`] 基础上额外提供 `new_uuid(namespace)` 构造器,
//!   裸值是随机 UUID v4 字符串,便于本地音乐这类需要随机 ID 的来源。

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

use serde::{Deserialize, Serialize};

#[doc(hidden)]
pub use uuid;

/// 源内的裸标识值。
///
/// 某个 channel 后端实际使用的 id 字符串(如网易云的数字串)。**不含来源信息**——
/// 来源由包裹它的 ID newtype 的 namespace 维护。序列化为透明字符串。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdString(String);

impl IdString {
    /// 从任意可转 `String` 的值构造。
    #[inline]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 裸值的 `&str` 视图。
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for IdString {
    #[inline]
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for IdString {
    #[inline]
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for IdString {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// 生成一个结构化 ID newtype:`{ namespace: $ns, value: IdString }`。
///
/// 展开内容:
/// - `pub struct $name { namespace: $ns, value: IdString }`(私有字段,只能经 `new` 构造)
/// - `Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize`
/// - `new(namespace, value)` / `namespace()` / `value()` / `as_str()` / `qualified()`
/// - `Display`(输出**裸值**,与历史 `{id}` 格式点兼容)
///
/// # 使用要求
///
/// - 调用方需要 `serde` 在依赖图中可用(宏展开使用 `::serde::*` 绝对路径)。
/// - `$ns` 必须 `Copy + Debug`(`namespace()` 按值返回、`qualified()` 用 `{:?}` 拼接)。
#[macro_export]
macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $ns:ty) => {
        $(#[$meta])*
        #[doc = concat!("`", stringify!($name), "` — 结构化 ID(namespace + 裸值)。")]
        #[derive(
            Clone,
            Debug,
            PartialEq,
            Eq,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        pub struct $name {
            /// 来源命名空间——决定裸值在哪个 channel 内解释。
            namespace: $ns,

            /// 源内裸标识值。
            value: $crate::IdString,
        }

        impl $name {
            /// 用 namespace + 裸值构造一个新 ID。
            #[inline]
            pub fn new(
                namespace: $ns,
                value: impl ::std::convert::Into<::std::string::String>,
            ) -> Self {
                Self {
                    namespace,
                    value: $crate::IdString::new(value),
                }
            }

            /// 返回来源 namespace。
            #[inline]
            pub fn namespace(&self) -> $ns {
                self.namespace
            }

            /// 源内裸值的 `&str` 视图(喂给 channel 后端 / 日志)。
            #[inline]
            pub fn value(&self) -> &str {
                self.value.as_str()
            }

            /// [`Self::value`] 的别名,保持历史调用点行为(返回裸值)。
            #[inline]
            pub fn as_str(&self) -> &str {
                self.value.as_str()
            }

            /// 全局唯一的限定字符串 `namespace:value`(任务去重键等用)。
            #[inline]
            pub fn qualified(&self) -> ::std::string::String {
                ::std::format!("{:?}:{}", self.namespace, self.value)
            }
        }

        impl ::std::fmt::Display for $name {
            #[inline]
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                ::std::fmt::Display::fmt(&self.value, f)
            }
        }
    };
}

/// 在 [`define_id!`] 基础上加 UUID 构造器。
#[macro_export]
macro_rules! define_uuid {
    ($(#[$meta:meta])* $name:ident, $ns:ty) => {
        $crate::define_id!($(#[$meta])* $name, $ns);

        impl $name {
            /// 用给定 namespace + 随机 UUID v4 裸值构造一个新 ID。
            #[inline]
            pub fn new_uuid(namespace: $ns) -> Self {
                Self::new(namespace, $crate::uuid::Uuid::new_v4().to_string())
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    /// 测试用 namespace(模拟 `SourceKind`:Copy + Debug + serde)。
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub(crate) enum Ns {
        /// 变体 A。
        A,
        /// 变体 B。
        B,
    }

    define_id!(SongId, Ns);
    define_uuid!(LocalSongId, Ns);

    #[test]
    fn new_and_accessors() {
        let id = SongId::new(Ns::A, "abc");
        assert_eq!(id.value(), "abc");
        assert_eq!(id.as_str(), "abc");
        assert_eq!(id.namespace(), Ns::A);
        assert_eq!(id.to_string(), "abc");
        assert_eq!(id.qualified(), "A:abc");
    }

    #[test]
    fn distinct_namespace_distinct_id() {
        let a = SongId::new(Ns::A, "1");
        let b = SongId::new(Ns::B, "1");
        assert_ne!(a, b, "同裸值不同 namespace 必须不相等");
        assert_ne!(a.qualified(), b.qualified());
    }

    #[test]
    fn serde_roundtrip() {
        let id = SongId::new(Ns::A, "xyz");
        let s = serde_json::to_string(&id).unwrap();
        assert!(s.contains("xyz"), "序列化应含裸值: {s}");
        let back: SongId = serde_json::from_str(&s).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn uuid_constructor_produces_valid_uuid() {
        let id = LocalSongId::new_uuid(Ns::B);
        assert_eq!(id.namespace(), Ns::B);
        let parsed = uuid::Uuid::parse_str(id.value());
        assert!(parsed.is_ok(), "expected valid UUID, got {id:?}");
        // 同时覆盖 as_str / qualified(裸值 / 限定形式)。
        assert_eq!(id.as_str(), id.value());
        assert!(id.qualified().starts_with("B:"));
    }
}
