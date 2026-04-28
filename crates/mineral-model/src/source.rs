use serde::{Deserialize, Serialize};

/// 标识一份资源(歌曲、专辑……)的来源 channel。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Netease,
    Local,
}
