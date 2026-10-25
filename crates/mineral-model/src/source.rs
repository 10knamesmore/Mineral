use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use rustc_hash::FxHashSet;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// 标识一份资源(歌曲、专辑……)的来源(source)
#[derive(Clone, Copy)]
pub struct SourceKind {
    /// 稳定标识——跨进程身份、serde 表示就是它。
    name: &'static str,

    /// UI 展示名(可含字形图标)。
    label: &'static str,
}

impl SourceKind {
    /// Apple Music.
    pub const APPLE_MUSIC: Self = Self {
        name: "am",
        label: " Apple",
    };

    /// 网易云音乐。
    pub const NETEASE: Self = Self {
        name: "netease",
        label: "󰎆 NetEase",
    };

    /// 本地文件系统(用户的 music 目录)。
    pub const LOCAL: Self = Self {
        name: "local",
        label: " local",
    };

    /// 哔哩哔哩。
    pub const BILIBILI: Self = Self {
        name: "bilibili",
        label: "󰟴 BiliBili",
    };

    /// Mineral 自身——跨源聚合内容(如全源收藏合集)的挂靠来源,无网络后端。
    pub const MINERAL: Self = Self {
        name: "mineral",
        label: "◆ Mineral",
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
    ///   命中内置常量则返回之;未知名 intern 成 `&'static str` 并给默认展示(label = name)。
    pub fn from_name(name: &str) -> Self {
        match name {
            "am" => Self::APPLE_MUSIC,
            "netease" => Self::NETEASE,
            "local" => Self::LOCAL,
            "bilibili" => Self::BILIBILI,
            "mineral" => Self::MINERAL,
            other => {
                let interned = intern(other);
                Self::from_static(interned, interned)
            }
        }
    }
}

/// 把一个运行时字符串固化成 `&'static str`;相同内容复用池中条目。
///
/// 每个不同的未知来源名都会保留到进程结束,数量不受本函数限制。
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
}
