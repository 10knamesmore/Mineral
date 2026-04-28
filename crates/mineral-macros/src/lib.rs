//! 提供 ID newtype 的样板宏。
//!
//! - [`define_id!`] 生成一个 String-backed 的 newtype,带 serde transparent、
//!   常用 `From`/`Display`/`FromStr` 实现。
//! - [`define_uuid!`] 在 `define_id!` 基础上额外提供 `new_uuid()` 构造器,
//!   内部值仍是 String,便于和 `define_id!` 类型零成本互转。

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

#[doc(hidden)]
pub use uuid;

/// 生成一个 String-backed 的 newtype ID。
///
/// 展开内容:
/// - `pub struct $name(pub String);`
/// - `Clone, Debug, PartialEq, Eq, Hash, Default`
/// - `serde::Serialize` / `serde::Deserialize`(`#[serde(transparent)]`)
/// - `From<String>` / `From<&str>` / `From<$name> for String`
/// - `Display` / `FromStr<Err = Infallible>` / `Deref<Target = str>`
/// - `new` / `as_str` / `into_string`
///
/// # 使用要求
///
/// 调用方需要确保 `serde` 在依赖图中可用——宏展开使用 `::serde::*` 绝对路径。
#[macro_export]
macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[doc = concat!("`", stringify!($name), "` — String-backed ID newtype。")]
        #[derive(
            Clone,
            Debug,
            Default,
            PartialEq,
            Eq,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(
            /// 内部 String 值。
            pub ::std::string::String,
        );

        impl $name {
            /// 从任何可转 `String` 的类型构造一个新 ID。
            #[inline]
            pub fn new(s: impl ::std::convert::Into<::std::string::String>) -> Self {
                Self(s.into())
            }

            /// 返回内部字符串的 `&str` 视图。
            #[inline]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// 拆出内部 `String`,消费 self。
            #[inline]
            pub fn into_string(self) -> ::std::string::String {
                self.0
            }
        }

        impl ::std::convert::From<::std::string::String> for $name {
            #[inline]
            fn from(value: ::std::string::String) -> Self {
                Self(value)
            }
        }

        impl ::std::convert::From<&str> for $name {
            #[inline]
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl ::std::convert::From<$name> for ::std::string::String {
            #[inline]
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl ::std::ops::Deref for $name {
            type Target = ::std::primitive::str;

            #[inline]
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            #[inline]
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ::std::convert::Infallible;

            #[inline]
            fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
                ::std::result::Result::Ok(Self(s.to_owned()))
            }
        }
    };
}

/// 在 [`define_id!`] 基础上加 UUID 构造器。
#[macro_export]
macro_rules! define_uuid {
    ($(#[$meta:meta])* $name:ident) => {
        $crate::define_id!($(#[$meta])* $name);

        impl $name {
            /// 生成一个新的、由 UUID v4 字符串支撑的 ID。
            #[inline]
            pub fn new_uuid() -> Self {
                Self($crate::uuid::Uuid::new_v4().to_string())
            }
        }
    };
}

#[cfg(test)]
mod tests {
    define_id!(SongId);
    define_uuid!(LocalSongId);

    #[test]
    fn from_and_display() {
        let id = SongId::from("abc");
        assert_eq!(id.as_str(), "abc");
        assert_eq!(&*id, "abc");
        assert_eq!(id.to_string(), "abc");
        assert_eq!(String::from(id), String::from("abc"));
    }

    #[test]
    fn serde_is_transparent() {
        let id = SongId::new("xyz");
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(s, "\"xyz\"");
        let back: SongId = serde_json::from_str(&s).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn fromstr_never_fails() {
        let id: SongId = "hello".parse().unwrap();
        assert_eq!(id.as_str(), "hello");
    }

    #[test]
    fn uuid_constructor_produces_valid_uuid() {
        let id = LocalSongId::new_uuid();
        let parsed = uuid::Uuid::parse_str(id.as_str());
        assert!(parsed.is_ok(), "expected valid UUID, got {id:?}");
    }
}
