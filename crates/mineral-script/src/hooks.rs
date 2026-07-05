//! 同步拦截 hook(`before_stream` / `before_download`)的类型面:拦截点
//! 类别、提交点口味、per-kind 入参快照与裁决结果。纯数据,不碰播放 / 下载
//! 执行面;往返管线在 [`ScriptSender`](crate::ScriptSender) 的类型化拦截
//! 入口(daemon 侧)与 dispatch 层(脚本侧)。
//!
//! 每个拦截点各有一个 ctx struct(字段集互不迁就),新增拦截点 = 新 struct +
//! 新消息变体 + 新发送入口,不动既有类型。

use mineral_model::{AudioFormat, BitRate, MediaUrl, PlayUrl, Song, StreamLayout};

/// 拦截点类别(注册名 / 回调桶键)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookKind {
    /// 一首歌走向「开播」的提交点:即时起播前 / gapless 预取武装前
    /// (两个提交点共用本类别,由 [`BeforeStreamCtx`] 的 `mode` 区分口味)。
    BeforeStream,

    /// 下载直链取得后、写盘前。
    BeforeDownload,
}

impl HookKind {
    /// 全部拦截点(`mineral.hook` 错误信息 / meta 守卫测试用)。
    pub const ALL: [Self; 2] = [Self::BeforeStream, Self::BeforeDownload];

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
            "before_stream" => Some(Self::BeforeStream),
            "before_download" => Some(Self::BeforeDownload),
            _ => None,
        }
    }

    /// hook 名字符串(Lua 侧注册名)。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeStream => "before_stream",
            Self::BeforeDownload => "before_download",
        }
    }
}

/// 提交点口味:同一个决策 hook 在哪个提交点 fire。
///
/// host 按口味给预算并解释裁决(`Prefetch` 有整段预取窗口的异步预算、`Skip` 落为
/// 否决预排;`Immediate` 预算短、`Skip` 落为推进),**脚本逻辑可与口味无关**
/// (简单脚本不必区分)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookMode {
    /// 即时:当前歌 URL 解析就绪、起播前(手动点播 / 边界兜底);下载链路恒为此口味。
    Immediate,

    /// 预取:gapless 预取的下一首武装进引擎 next 槽之前(曲尾窗口内,关键路径外)。
    Prefetch,
}

impl HookMode {
    /// 口味字符串(Lua ctx 的 `mode` 字段)。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Prefetch => "prefetch",
        }
    }
}

/// `before_stream` 的入参快照(只读,跨线程 move 给脚本线程)。
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct BeforeStreamCtx {
    /// 触发拦截的歌。
    song: Box<Song>,

    /// 宿主解析出的原始播放 URL;`None` = 解析失败(取链失败 / 灰歌),
    /// 即「unplayable」信号——脚本可改写顶入可播流。
    original: Option<Box<PlayUrl>>,

    /// 提交点口味。
    mode: HookMode,
}

impl BeforeStreamCtx {
    /// 打包入参快照。
    ///
    /// # Params:
    ///   - `song`: 触发歌
    ///   - `mode`: 提交点口味
    ///   - `original`: 原始 URL;`None` = 无可播 URL(unplayable)
    #[must_use]
    pub fn new(song: Song, mode: HookMode, original: Option<PlayUrl>) -> Self {
        Self {
            song: Box::new(song),
            original: original.map(Box::new),
            mode,
        }
    }

    /// 触发歌(只读)。
    #[must_use]
    pub fn song(&self) -> &Song {
        &self.song
    }

    /// 原始 URL / 音质(只读);`None` = 无可播 URL。
    #[must_use]
    pub fn original(&self) -> Option<&PlayUrl> {
        self.original.as_deref()
    }

    /// 提交点口味。
    #[must_use]
    pub fn mode(&self) -> HookMode {
        self.mode
    }

    /// 是否「无可播 URL」(取链失败 / 灰歌)——`original` 缺席的便利投影。
    #[must_use]
    pub fn unplayable(&self) -> bool {
        self.original.is_none()
    }
}

/// `before_download` 的入参快照(只读,跨线程 move 给脚本线程)。
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct BeforeDownloadCtx {
    /// 待下载的歌。
    song: Box<Song>,

    /// 取到的下载直链;`None` = 宿主没解析出直链。
    original: Option<Box<PlayUrl>>,
}

impl BeforeDownloadCtx {
    /// 打包入参快照。
    ///
    /// # Params:
    ///   - `song`: 待下载歌
    ///   - `original`: 下载直链;`None` = 无直链
    #[must_use]
    pub fn new(song: Song, original: Option<PlayUrl>) -> Self {
        Self {
            song: Box::new(song),
            original: original.map(Box::new),
        }
    }

    /// 待下载歌(只读)。
    #[must_use]
    pub fn song(&self) -> &Song {
        &self.song
    }

    /// 下载直链 / 音质(只读);`None` = 无直链。
    #[must_use]
    pub fn original(&self) -> Option<&PlayUrl> {
        self.original.as_deref()
    }

    /// 是否无直链——`original` 缺席的便利投影。
    #[must_use]
    pub fn unplayable(&self) -> bool {
        self.original.is_none()
    }
}

/// 一次同步拦截的裁决结果。
///
/// 脚本回调用返回值表达(`nil` 放行 / table 改写 / `false` 或 `{skip=...}`
/// 跳过 / [`mineral.DEFER`](crate) 延迟稍后经 `ctx.resolve` 补交),dispatch 层
/// 收敛成本枚举;超时 / 线程退出 / Lua 错误一律按 [`Self::Continue`] 放行
/// (拦截失败不致命)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookDecision {
    /// 放行,沿用原 URL / 原音质。
    Continue,

    /// 用脚本改写后的 URL / 音质继续(多源 fallback 场景)。
    Rewrite(RewriteSpec),

    /// 跳过本次播放 / 下载;宿主据此降级(即时口味推进下一首 / 预取口味
    /// 否决预排 / 下载记 skip)。
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

    /// 改写后的取流请求头(顶换的流若需鉴权/防盗链头,随之带上);`None` = 不改头。
    pub(crate) stream_headers: Option<Vec<(String, String)>>,

    /// 改写后的流容器布局;`None` = 脚本没指定(改 URL 时由 apply 层定安全默认)。
    /// 当改写把 URL 顶换成不同容器的流时,脚本据目标布局设置(分片流置 `Chunked` 让播放层流式
    /// 打开、直链置 `Contiguous` 保留 seek)。
    pub(crate) layout: Option<StreamLayout>,

    /// 顶换流的实测码率(bps);`None` = 脚本没给(展示层显 0)。纯展示元信息,
    /// 不参与播放决策——补救脚本从 `library.song_url` 拿到真值时透传。
    pub(crate) bitrate_bps: Option<u32>,

    /// 顶换流的容器格式;`None` = 脚本没给(展示层显缺失)。同为纯展示元信息。
    pub(crate) format: Option<AudioFormat>,
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

    /// 改写后的流容器布局(只读);`None` = 脚本未指定(apply 层定默认)。
    #[must_use]
    pub fn layout(&self) -> Option<StreamLayout> {
        self.layout
    }

    /// 顶换流的实测码率(bps,只读);`None` = 脚本未提供。
    #[must_use]
    pub fn bitrate_bps(&self) -> Option<u32> {
        self.bitrate_bps
    }

    /// 顶换流的容器格式(只读);`None` = 脚本未提供。
    #[must_use]
    pub fn format(&self) -> Option<&AudioFormat> {
        self.format.as_ref()
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
