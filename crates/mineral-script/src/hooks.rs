//! 同步拦截 hook(`before_play` / `before_download`)的类型面:拦截点
//! 类别、入参快照与裁决结果。纯数据,不碰播放 / 下载执行面;往返管线在
//! [`ScriptSender::intercept`](crate::ScriptSender::intercept)(daemon 侧)
//! 与 dispatch 层(脚本侧)。

use mineral_model::{BitRate, MediaUrl, PlayUrl, Song};

/// 拦截点类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookKind {
    /// 播放 URL 解析后、起播前。
    BeforePlay,

    /// 下载直链取得后、写盘前。
    BeforeDownload,
}

impl HookKind {
    /// 全部拦截点(`mineral.hook` 错误信息 / meta 守卫测试用)。
    pub const ALL: [Self; 2] = [Self::BeforePlay, Self::BeforeDownload];

    /// 按 hook 名解析(与 [`Self::as_str`] 对偶);未知名为 `None`。
    ///
    /// # Params:
    ///   - `name`: hook 名字符串(脚本侧输入)
    ///
    /// # Return:
    ///   对应类别;未知名为 `None`,调用方报脚本错误。
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "before_play" => Some(Self::BeforePlay),
            "before_download" => Some(Self::BeforeDownload),
            _ => None,
        }
    }

    /// hook 名字符串(Lua 侧注册名)。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforePlay => "before_play",
            Self::BeforeDownload => "before_download",
        }
    }
}

/// 一次同步拦截的入参快照(只读,跨线程 move 给脚本线程)。
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct HookContext {
    /// 触发拦截的歌。
    song: Box<Song>,

    /// 宿主解析出的原始播放 URL(`before_play`)/ 下载直链(`before_download`)。
    original: Box<PlayUrl>,
}

impl HookContext {
    /// 打包入参快照。
    ///
    /// # Params:
    ///   - `song`: 触发歌
    ///   - `original`: 原始 URL
    #[must_use]
    pub fn new(song: Song, original: PlayUrl) -> Self {
        Self {
            song: Box::new(song),
            original: Box::new(original),
        }
    }

    /// 触发歌(只读)。
    #[must_use]
    pub fn song(&self) -> &Song {
        &self.song
    }

    /// 原始 URL / 音质(只读)。
    #[must_use]
    pub fn original(&self) -> &PlayUrl {
        &self.original
    }
}

/// 一次同步拦截的裁决结果。
///
/// 脚本回调用返回值表达(`nil` 放行 / table 改写 / `false` 或 `{skip=...}`
/// 跳过),dispatch 层收敛成本枚举;超时 / 线程退出 / Lua 错误一律按
/// [`Self::Continue`] 放行(拦截失败不致命)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookDecision {
    /// 放行,沿用原 URL / 原音质。
    Continue,

    /// 用脚本改写后的 URL / 音质继续(多源 fallback 场景)。
    Rewrite(RewriteSpec),

    /// 跳过本次播放 / 下载;宿主据此降级(播放跳下一首 / 下载记 skip)。
    Skip {
        /// 跳过原因(toast + 日志,人读)。
        reason: String,
    },
}

/// 脚本的改写意图(结构化;Lua 字符串在 dispatch 层边界解析)。
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewriteSpec {
    /// 改写后的播放地址;`None` = 不改 URL。
    pub(crate) new_url: Option<MediaUrl>,

    /// 改写后的目标音质;`None` = 不改音质。
    pub(crate) new_quality: Option<BitRate>,

    /// 改写后的取流请求头(如 B站 baseUrl 顶替需 `Referer`);`None` = 不改头。
    pub(crate) stream_headers: Option<Vec<(String, String)>>,
}

impl RewriteSpec {
    /// 改写后的播放地址(只读)。
    #[must_use]
    pub fn new_url(&self) -> Option<&MediaUrl> {
        self.new_url.as_ref()
    }

    /// 改写后的音质(只读)。
    #[must_use]
    pub fn new_quality(&self) -> Option<BitRate> {
        self.new_quality
    }

    /// 改写后的取流请求头(只读);`None` = 不改头。
    #[must_use]
    pub fn stream_headers(&self) -> Option<&[(String, String)]> {
        self.stream_headers.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::HookKind;

    #[test]
    fn meta_stub_hook_name_alias_matches_rust() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        // meta/mineral.lua 的 `mineral.HookName` 字符串枚举必须与
        // Rust 侧 `as_str` 的全部取值逐字一致(顺序也钉死)。
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        let literals = HookKind::ALL
            .map(|kind| format!("\"{}\"", kind.as_str()))
            .join("|");
        let alias = format!("---@alias mineral.HookName {literals}");
        assert!(
            meta.contains(&alias),
            "meta stub 缺少与 Rust 一致的别名行:`{alias}`"
        );
        Ok(())
    }
}
