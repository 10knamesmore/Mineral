use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// 一个来源 channel 在 UI 上的语义调色角色(**主题无关**)。
///
/// 来源只声明角色,具体颜色由 TUI 用当前主题解析——避免插件硬编码颜色跟主题打架。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaletteRole {
    /// 品牌强调色(主云源)。
    Accent,
    /// 中性弱化色(本地源)。
    Muted,
    /// 最弱的占位色(测试 / 未知源)。
    Faint,
}

/// 标识一份资源(歌曲、专辑……)的来源 channel——强类型、开放集合、自描述。
///
/// 仿 `http::StatusCode` 的「newtype + 关联常量」:内置源是常量
/// ([`SourceKind::NETEASE`] 等),将来插件源可经 [`SourceKind::from_static`] 在运行时铸造。
/// 全字段都是 `Copy`,故可零成本嵌进每个 ID 的 namespace。
///
/// **身份只看 [`name`](SourceKind::name)**——`label` / `palette` 是随 `name` 走的展示
/// 元数据,**不参与**相等 / 哈希 / 序列化(序列化只写 `name`,反序列化按 `name` 解析回
/// 完整定义)。这样同一来源无论从哪构造,相等性与 HashMap key 行为都一致。
#[derive(Clone, Copy)]
pub struct SourceKind {
    /// 稳定标识——跨进程身份、serde 表示就是它。
    name: &'static str,

    /// UI 展示名(可含字形图标)。
    label: &'static str,

    /// UI 调色角色(主题无关)。
    palette: PaletteRole,
}

impl SourceKind {
    /// 网易云音乐。
    pub const NETEASE: Self = Self {
        name: "netease",
        label: "♫ netease",
        palette: PaletteRole::Accent,
    };

    /// 本地文件系统(用户的 music 目录)。
    pub const LOCAL: Self = Self {
        name: "local",
        label: "□ local",
        palette: PaletteRole::Muted,
    };

    /// 占位 / 测试用伪 channel — 仅在启用 `mock` feature 时存在。
    #[cfg(feature = "mock")]
    pub const MOCK: Self = Self {
        name: "mock",
        label: "▒ mock",
        palette: PaletteRole::Faint,
    };

    /// 铸造一个来源(插件 / 运行时扩展用)。
    ///
    /// # Params:
    ///   - `name`: 稳定标识(serde / 身份用)
    ///   - `label`: UI 展示名
    ///   - `palette`: UI 调色角色
    ///
    /// # Return:
    ///   对应的 [`SourceKind`]。
    pub const fn from_static(
        name: &'static str,
        label: &'static str,
        palette: PaletteRole,
    ) -> Self {
        Self {
            name,
            label,
            palette,
        }
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

    /// UI 调色角色(主题无关)。
    #[inline]
    pub const fn palette(&self) -> PaletteRole {
        self.palette
    }

    /// 按稳定 `name` 解析回一个来源。
    ///
    /// # Params:
    ///   - `name`: 稳定标识(与 [`name`](Self::name) 对称)
    ///
    /// # Return:
    ///   命中内置常量则返回之;未知名(将来插件) intern 成 `&'static str` 并给默认展示(label = name、`Faint`)。
    pub fn from_name(name: &str) -> Self {
        match name {
            "netease" => Self::NETEASE,
            "local" => Self::LOCAL,
            #[cfg(feature = "mock")]
            "mock" => Self::MOCK,
            other => {
                let interned = intern(other);
                Self::from_static(interned, interned, PaletteRole::Faint)
            }
        }
    }
}

/// 把一个运行时字符串固化成 `&'static str`,带去重池避免重复泄漏。
///
/// 仅在反序列化遇到未知来源名时走到;来源集合极小,泄漏有界。
fn intern(s: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashSet::new()));
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
    use std::collections::HashMap;

    use super::{PaletteRole, SourceKind};

    /// 身份只认 `name`:label 不同也相等,HashMap key 行为一致。
    #[test]
    fn identity_is_name_only() {
        let canonical = SourceKind::NETEASE;
        let relabeled = SourceKind::from_static("netease", "别的 label", PaletteRole::Faint);
        assert_eq!(canonical, relabeled, "同 name 即同一来源");
        let mut by_source = HashMap::new();
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
        assert_eq!(s.palette(), PaletteRole::Accent);
        assert_eq!(format!("{s:?}"), "netease");
    }

    /// 已知名字解析回内置常量;未知名字给默认展示(label = name、`Faint`)。
    #[test]
    fn from_name_known_and_unknown() {
        assert_eq!(SourceKind::from_name("local"), SourceKind::LOCAL);
        assert_eq!(SourceKind::from_name("local").palette(), PaletteRole::Muted);

        let plugin = SourceKind::from_name("myplugin");
        assert_eq!(plugin.name(), "myplugin");
        assert_eq!(plugin.label(), "myplugin");
        assert_eq!(plugin.palette(), PaletteRole::Faint);
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
}
