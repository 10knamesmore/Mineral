use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use rustc_hash::FxHashSet;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// 标识一份资源(歌曲、专辑……)的来源 channel——强类型、开放集合、自描述。
///
/// 仿 `http::StatusCode` 的「newtype + 关联常量」:内置源是常量
/// ([`SourceKind::NETEASE`] 等),将来插件源可经 [`SourceKind::from_static`] 在运行时铸造。
/// 全字段都是 `Copy`,故可零成本嵌进每个 ID 的 namespace。
///
/// **身份只看 [`name`](SourceKind::name)**——`label` 是随 `name` 走的展示元数据,**不参与**
/// 相等 / 哈希 / 序列化(序列化只写 `name`,反序列化按 `name` 解析回完整定义)。这样同一来源
/// 无论从哪构造,相等性与 HashMap key 行为都一致。
///
/// 来源徽标**颜色不在此声明**:它是 per-source 的配置(`sources.<name>.color`),由 TUI 按
/// `name` 从配置解析(命中的走配置色 / 未配置的走中立兜底)——避免把一个闭合调色枚举强塞进
/// 开放的来源集合。
#[derive(Clone, Copy)]
pub struct SourceKind {
    /// 稳定标识——跨进程身份、serde 表示就是它。
    name: &'static str,

    /// UI 展示名(可含字形图标)。
    label: &'static str,
}

impl SourceKind {
    /// 网易云音乐。
    pub const NETEASE: Self = Self {
        name: "netease",
        label: "♫ netease",
    };

    /// 本地文件系统(用户的 music 目录)。
    pub const LOCAL: Self = Self {
        name: "local",
        label: "□ local",
    };

    /// 哔哩哔哩。
    pub const BILIBILI: Self = Self {
        name: "bilibili",
        label: "▶ bilibili",
    };

    /// Mineral 自身——跨源聚合内容(如全源收藏合集)的挂靠来源,无网络后端。
    pub const MINERAL: Self = Self {
        name: "mineral",
        label: "◆ mineral",
    };

    /// 占位 / 测试用伪 channel — 仅在启用 `mock` feature 时存在。
    #[cfg(feature = "mock")]
    pub const MOCK: Self = Self {
        name: "mock",
        label: "▒ mock",
    };

    /// 铸造一个来源(插件 / 运行时扩展用)。
    ///
    /// # Params:
    ///   - `name`: 稳定标识(serde / 身份用)
    ///   - `label`: UI 展示名
    ///
    /// # Return:
    ///   对应的 [`SourceKind`]。
    pub const fn from_static(name: &'static str, label: &'static str) -> Self {
        Self { name, label }
    }

    /// 稳定标识(serde / 身份 / 日志用)。
    #[inline]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// UI 展示名(可含字形图标)。
    #[inline]
    pub const fn label(&self) -> &'static str {
        self.label
    }

    /// 按稳定 `name` 解析回一个来源。
    ///
    /// # Params:
    ///   - `name`: 稳定标识(与 [`name`](Self::name) 对称)
    ///
    /// # Return:
    ///   命中内置常量则返回之;未知名(将来插件) intern 成 `&'static str` 并给默认展示(label = name)。
    pub fn from_name(name: &str) -> Self {
        match name {
            "netease" => Self::NETEASE,
            "local" => Self::LOCAL,
            "bilibili" => Self::BILIBILI,
            "mineral" => Self::MINERAL,
            #[cfg(feature = "mock")]
            "mock" => Self::MOCK,
            other => {
                let interned = intern(other);
                Self::from_static(interned, interned)
            }
        }
    }
}

/// 把一个运行时字符串固化成 `&'static str`,带去重池避免重复泄漏。
///
/// 仅在反序列化遇到未知来源名时走到;来源集合极小,泄漏有界。
fn intern(s: &str) -> &'static str {
    static POOL: OnceLock<Mutex<FxHashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(FxHashSet::default()));
    // 中毒锁也能取回内部数据,不 panic。
    let mut guard = pool
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = guard.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

impl PartialEq for SourceKind {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for SourceKind {}

impl Hash for SourceKind {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl std::fmt::Debug for SourceKind {
    /// 只打 `name`——让 ID 的 `qualified()`/日志保持 `netease:123` 这样的干净形态。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

impl Serialize for SourceKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name)
    }
}

impl<'de> Deserialize<'de> for SourceKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name))
    }
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashMap;

    use super::SourceKind;

    /// 身份只认 `name`:label 不同也相等,HashMap key 行为一致。
    #[test]
    fn identity_is_name_only() {
        let canonical = SourceKind::NETEASE;
        let relabeled = SourceKind::from_static("netease", "别的 label");
        assert_eq!(canonical, relabeled, "同 name 即同一来源");
        let mut by_source = FxHashMap::default();
        by_source.insert(canonical, 1_u8);
        assert_eq!(
            by_source.get(&relabeled),
            Some(&1),
            "HashMap key 也只看 name"
        );
    }

    /// 访问器与 Debug(Debug 只打 name,保证 qualified() 干净)。
    #[test]
    fn accessors_and_debug() {
        let s = SourceKind::NETEASE;
        assert_eq!(s.name(), "netease");
        assert_eq!(s.label(), "♫ netease");
        assert_eq!(format!("{s:?}"), "netease");
    }

    /// 已知名字解析回内置常量;未知名字给默认展示(label = name)。
    #[test]
    fn from_name_known_and_unknown() {
        assert_eq!(SourceKind::from_name("local"), SourceKind::LOCAL);

        let plugin = SourceKind::from_name("myplugin");
        assert_eq!(plugin.name(), "myplugin");
        assert_eq!(plugin.label(), "myplugin");
    }

    /// 内置名字往返:`from_name` 命中常量。
    #[test]
    fn from_name_roundtrips_builtins() {
        assert_eq!(SourceKind::from_name("netease"), SourceKind::NETEASE);
        assert_eq!(SourceKind::from_name("local"), SourceKind::LOCAL);
    }

    /// 未知名字 intern,`name()` 仍可取回原值。
    #[test]
    fn from_name_interns_unknown() {
        let plugin = SourceKind::from_name("spotify");
        assert_eq!(plugin.name(), "spotify");
    }

    /// mineral 聚合源是内置常量:from_name 命中(非 intern 兜底),label 带字形。
    #[test]
    fn mineral_is_builtin() {
        assert_eq!(SourceKind::from_name("mineral"), SourceKind::MINERAL);
        assert_eq!(SourceKind::MINERAL.name(), "mineral");
        assert_eq!(SourceKind::MINERAL.label(), "◆ mineral");
    }
}
