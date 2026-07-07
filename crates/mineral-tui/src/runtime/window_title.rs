//! TUI 窗口标题：把当前播放态实时写进终端任务栏 / tab 标题。
//!
//! 标题是既有 client 状态的纯派生，按优先级取：断连（client 强制）> 脚本旋钮
//! 覆盖 > 结构化四态模板（播放 / 暂停 / 空闲各一套，`StateIcon` 段按当前态解析
//! 图标字形）。写入走 crossterm `SetTitle` + 变化检测，避免每帧刷 OSC；写前抹掉
//! 控制字符，杜绝远端元数据里的转义序列注入。

use std::borrow::Cow;
use std::io;

use crossterm::execute;
use crossterm::terminal::SetTitle;
use mineral_config::{TimeFormat, TitleField, TitleSegment, WindowTitleConfig};
use mineral_model::Song;

/// 渲染窗口标题所需的当帧上下文（既有状态的只读投影）。
pub(crate) struct TitleContext<'a> {
    /// 当前在播歌曲（空闲态为 `None`）。
    pub song: Option<&'a Song>,

    /// 是否正在播放（来自 audio snapshot）。
    pub playing: bool,

    /// 是否与 daemon 保持连接。
    pub connected: bool,

    /// 当前播放进度（ms）。
    pub position_ms: u64,

    /// 当前曲目全长（ms）；`None` = 未知（未探出且元数据缺），标题里省略总长段。
    pub duration_ms: Option<u64>,

    /// 当前歌词行文本，已按 [`sync_trust`](crate::runtime::playback::Playback::sync_trust)
    /// 过滤（失真档为 `None`）。
    pub lyric: Option<&'a str>,

    /// 脚本 `mineral.ui.window_title` 覆盖当前值（自渲染整串;`None` = 无覆盖）。
    pub override_text: Option<&'a str>,
}

/// 窗口标题的状态机 + 变化检测。
pub(crate) struct WindowTitle {
    /// `tui.window_title` 段配置（总开关 / 四态图标 / 四态模板）。
    cfg: WindowTitleConfig,

    /// 是否有任一模板引用 `Lyric` 字段。无引用时调用方可跳过每帧的歌词行拼接
    /// （拼接要按时间轴定位当前行并拥有化文本，无谓的每帧分配）。
    wants_lyric: bool,

    /// 上一次实际写给终端的标题。`None` 表示尚未写入或已禁用。
    last_title: Option<String>,
}

impl WindowTitle {
    /// 从配置初始化。
    ///
    /// # Params:
    ///   - `cfg`: `tui.window_title` 段配置。
    ///
    /// # Return:
    ///   新的窗口标题管理器。
    pub(crate) fn new(cfg: &WindowTitleConfig) -> Self {
        let wants_lyric = references_lyric(cfg.template())
            || references_lyric(cfg.idle())
            || references_lyric(cfg.disconnected());
        Self {
            cfg: cfg.clone(),
            wants_lyric,
            last_title: None,
        }
    }

    /// 是否有任一模板引用歌词字段。调用方据此决定是否每帧拼接当前歌词行。
    ///
    /// # Return:
    ///   有引用为 `true`。
    pub(crate) fn wants_lyric(&self) -> bool {
        self.wants_lyric
    }

    /// 标题系统是否启用（总开关）。禁用时 [`Self::render`] 恒 `None`；调用方据此决定
    /// 是否需要 push 终端标题栈（禁用则不碰终端标题）。
    ///
    /// # Return:
    ///   启用为 `true`。
    pub(crate) fn enabled(&self) -> bool {
        *self.cfg.enabled()
    }

    /// 根据当前上下文按优先级渲染标题字符串。
    ///
    /// # Params:
    ///   - `ctx`: 当帧上下文。
    ///
    /// # Return:
    ///   应显示的新标题；禁用时返回 `None`。
    fn render(&self, ctx: &TitleContext<'_>) -> Option<String> {
        if !*self.cfg.enabled() {
            return None;
        }
        let icons = self.cfg.icons();
        // 断连优先且 client 强制：此时脚本通道已死，旋钮值 stale，忽略。
        if !ctx.connected {
            return Some(fold(self.cfg.disconnected(), icons.disconnected(), ctx));
        }
        // 旋钮覆盖胜过结构化模板（脚本自渲染整串）。
        if let Some(text) = ctx.override_text {
            return Some(text.to_owned());
        }
        if ctx.song.is_none() {
            return Some(fold(self.cfg.idle(), icons.idle(), ctx));
        }
        let icon = if ctx.playing {
            icons.playing()
        } else {
            icons.paused()
        };
        Some(fold(self.cfg.template(), icon, ctx))
    }

    /// 上下文变化导致标题串变化时才写给终端。
    ///
    /// # Params:
    ///   - `ctx`: 当帧上下文。
    ///
    /// # Return:
    ///   写入操作可能的 IO 错误。
    pub(crate) fn update(&mut self, ctx: &TitleContext<'_>) -> io::Result<()> {
        let new_title = self.render(ctx);
        if new_title == self.last_title {
            return Ok(());
        }
        if let Some(title) = &new_title {
            // 标题里可能混入远端元数据（歌名 / 歌词），写前抹掉控制字符再发 OSC，
            // 杜绝 BEL / ESC 提前终止或另起转义序列（终端标题注入）。仅在标题真变时走一次。
            execute!(io::stdout(), SetTitle(sanitize_title(title)))?;
        }
        self.last_title = new_title;
        Ok(())
    }

    /// 仅用于测试：直接取上一次写给终端的标题。
    #[cfg(test)]
    fn last_title(&self) -> Option<&str> {
        self.last_title.as_deref()
    }
}

/// 模板中是否有段引用 `Lyric` 字段。
fn references_lyric(segments: &[TitleSegment]) -> bool {
    segments.iter().any(|s| {
        matches!(
            s,
            TitleSegment::Field {
                field: TitleField::Lyric,
                ..
            }
        )
    })
}

/// 抹掉标题里的控制字符（含换行 / BEL / ESC），逐个压成空格。标题是单行 OSC 载荷，
/// 任何控制符都可能被终端解读成转义序列的一部分（与既有多行简介的 `sanitize_controls`
/// 不同：那条保留换行做折行，标题要连换行一起抹）。
fn sanitize_title(title: &str) -> String {
    title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// fold 一套模板：`StateIcon{icon=true}` → 当前态字形 + 空格（`icon=false` 空段跳过）；
/// `Field` → 按字段取值，空值折叠（连同 prefix/suffix）；`Literal` 原样。
///
/// # Params:
///   - `template`: 段序列。
///   - `icon`: 当前态解析出的图标字形。
///   - `ctx`: 当帧上下文。
///
/// # Return:
///   fold 出的标题字符串。
fn fold(template: &[TitleSegment], icon: &str, ctx: &TitleContext<'_>) -> String {
    let mut out = String::new();
    for segment in template {
        match segment {
            TitleSegment::StateIcon { icon: enabled } => {
                if *enabled {
                    out.push_str(icon);
                    out.push(' ');
                }
            }
            TitleSegment::Field {
                field,
                prefix,
                suffix,
                format,
            } => {
                if let Some(value) = field_value(*field, format, ctx)
                    && !value.is_empty()
                {
                    out.push_str(prefix);
                    out.push_str(&value);
                    out.push_str(suffix);
                }
            }
            TitleSegment::Literal { text } => out.push_str(text),
        }
    }
    out
}

/// 取一个字段的渲染值（`None` = 该段折叠）。歌曲派生字段借用上下文里的串（零分配），
/// 只有时间字段（`Position` / `Duration`）现算故拥有化。
///
/// `Position` 恒渲染（0 → `00:00`）；`Duration` 为 0（未探出）折叠；歌曲派生字段
/// 无歌 / 无值折叠；`Lyric` 无当前行折叠。
fn field_value<'a>(
    field: TitleField,
    format: &TimeFormat,
    ctx: &TitleContext<'a>,
) -> Option<Cow<'a, str>> {
    match field {
        TitleField::Title => ctx.song.map(|s| Cow::Borrowed(s.name.as_str())),
        TitleField::Artist => ctx
            .song
            .and_then(|s| s.artists.first())
            .map(|a| Cow::Borrowed(a.name.as_str())),
        TitleField::Album => ctx
            .song
            .and_then(|s| s.album.as_ref())
            .map(|a| Cow::Borrowed(a.name.as_str())),
        TitleField::Source => ctx.song.map(|s| Cow::Borrowed(s.source().label())),
        TitleField::Position => Some(Cow::Owned(format.render(ctx.position_ms))),
        TitleField::Duration => ctx.duration_ms.map(|d| Cow::Owned(format.render(d))),
        TitleField::Lyric => ctx.lyric.map(Cow::Borrowed),
    }
}

#[cfg(test)]
mod tests {
    use mineral_config::{Config, TimeFormat, TimePreset, TitleField, TitleSegment};
    use mineral_model::Song;
    use mineral_test::{song, with_album, with_artist};

    use super::{TitleContext, WindowTitle, fold, sanitize_title};

    /// 造一个默认上下文（播放中、已连接、无进度 / 歌词 / 覆盖）。
    fn ctx(song: Option<&Song>) -> TitleContext<'_> {
        TitleContext {
            song,
            playing: true,
            connected: true,
            position_ms: 0,
            duration_ms: None,
            lyric: None,
            override_text: None,
        }
    }

    /// 一个纯时间字段段（clock 预设）。
    fn field(field: TitleField, prefix: &str, suffix: &str) -> TitleSegment {
        TitleSegment::Field {
            field,
            prefix: prefix.to_owned(),
            suffix: suffix.to_owned(),
            format: TimeFormat::default(),
        }
    }

    // ── 状态机 / 优先级：走 WindowTitle::new(默认配置) + render ──

    /// 默认模板渲染播放 / 暂停态：仅图标不同。
    #[test]
    fn default_template_playing_and_paused() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let wt = WindowTitle::new(cfg.tui().window_title());
        let s = with_artist(song("s"), "Artist Name");
        assert_eq!(
            wt.render(&ctx(Some(&s))),
            Some("⏸ s — Artist Name".to_owned())
        );
        let mut paused = ctx(Some(&s));
        paused.playing = false;
        assert_eq!(wt.render(&paused), Some("▶ s — Artist Name".to_owned()));
        Ok(())
    }

    /// 无艺人时 prefix 整段折叠，不残留 " — "。
    #[test]
    fn artist_prefix_folds_when_empty() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let wt = WindowTitle::new(cfg.tui().window_title());
        let s = song("s");
        assert_eq!(wt.render(&ctx(Some(&s))), Some("⏸ s".to_owned()));
        Ok(())
    }

    /// 空闲态走 idle 模板：`■ Mineral`。
    #[test]
    fn idle_state() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let wt = WindowTitle::new(cfg.tui().window_title());
        assert_eq!(wt.render(&ctx(None)), Some("■ Mineral".to_owned()));
        Ok(())
    }

    /// 断连态走 disconnected 模板，优先级最高（覆盖有歌态）。
    #[test]
    fn disconnected_overrides_all() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let wt = WindowTitle::new(cfg.tui().window_title());
        let s = with_artist(song("s"), "Artist");
        let mut c = ctx(Some(&s));
        c.connected = false;
        c.override_text = Some("脚本串"); // 断连时旋钮 stale，忽略
        assert_eq!(wt.render(&c), Some("⚠ Mineral".to_owned()));
        Ok(())
    }

    /// 旋钮覆盖胜过结构化模板（已连接时）。
    #[test]
    fn override_text_wins_over_template() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let wt = WindowTitle::new(cfg.tui().window_title());
        let s = with_artist(song("s"), "Artist");
        let mut c = ctx(Some(&s));
        c.override_text = Some("⠙ 自定义标题");
        assert_eq!(wt.render(&c), Some("⠙ 自定义标题".to_owned()));
        Ok(())
    }

    // ── fold 渲染细节：直接调 fold（自定义段无需构造完整配置）──

    /// StateIcon：`icon=true` 输出字形 + 空格，`icon=false` 空段跳过。
    #[test]
    fn state_icon_toggle() {
        let s = song("s");
        let c = ctx(Some(&s));
        assert_eq!(
            fold(&[TitleSegment::StateIcon { icon: true }], "▶", &c),
            "▶ "
        );
        assert_eq!(
            fold(&[TitleSegment::StateIcon { icon: false }], "▶", &c),
            ""
        );
    }

    /// position / duration 字段：clock 预设，duration=0 折叠、position=0 渲染 00:00。
    #[test]
    fn position_and_duration_clock() -> color_eyre::Result<()> {
        let template = [
            field(TitleField::Position, "", ""),
            field(TitleField::Duration, "/", ""),
        ];
        let s = song("s");
        let mut c = ctx(Some(&s));
        // duration 未知 → 折叠；position=0 → 00:00。
        assert_eq!(fold(&template, "", &c), "00:00");
        // 有进度 + 全长。
        c.position_ms = 83_000;
        c.duration_ms = Some(296_000);
        assert_eq!(fold(&template, "", &c), "01:23/04:56");
        Ok(())
    }

    /// source 字段渲染来源 label。
    #[test]
    fn source_field() -> color_eyre::Result<()> {
        let s = song("s"); // mineral_test::song → netease 命名空间
        assert_eq!(
            fold(&[field(TitleField::Source, "", "")], "", &ctx(Some(&s))),
            s.source().label()
        );
        Ok(())
    }

    /// lyric 字段：有当前行则渲染，无则折叠。
    #[test]
    fn lyric_field_renders_and_folds() -> color_eyre::Result<()> {
        let template = [field(TitleField::Lyric, "♪ ", "")];
        let s = song("s");
        let mut c = ctx(Some(&s));
        c.lyric = Some("这是当前歌词");
        assert_eq!(fold(&template, "", &c), "♪ 这是当前歌词");
        c.lyric = None;
        assert_eq!(fold(&template, "", &c), "", "无当前行整段折叠");
        Ok(())
    }

    /// 自定义时间格式串（pattern）。
    #[test]
    fn custom_pattern_format() -> color_eyre::Result<()> {
        let template = [TitleSegment::Field {
            field: TitleField::Position,
            prefix: String::new(),
            suffix: String::new(),
            format: TimeFormat::Pattern {
                pattern: "{m}分{ss}秒".to_owned(),
            },
        }];
        let s = song("s");
        let mut c = ctx(Some(&s));
        c.position_ms = 83_000;
        assert_eq!(fold(&template, "", &c), "1分23秒");
        Ok(())
    }

    /// album 字段与 seconds 预设。
    #[test]
    fn album_and_seconds() -> color_eyre::Result<()> {
        let template = [
            TitleSegment::Literal {
                text: "[".to_owned(),
            },
            field(TitleField::Album, "", ""),
            TitleSegment::Literal {
                text: "] ".to_owned(),
            },
            TitleSegment::Field {
                field: TitleField::Position,
                prefix: String::new(),
                suffix: "s".to_owned(),
                format: TimeFormat::Preset(TimePreset::Seconds),
            },
        ];
        let s = with_album(song("s"), "My Album");
        let mut c = ctx(Some(&s));
        c.position_ms = 42_000;
        assert_eq!(fold(&template, "", &c), "[My Album] 42s");
        Ok(())
    }

    // ── 开关 / 派生标志 / 消毒 / 变化检测 ──

    /// 禁用（enabled=false）时不产生任何标题。
    #[test]
    fn disabled_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            r#"return { tui = { window_title = { enabled = false } } }"#,
        )?;
        let (cfg, _warnings) = mineral_config::load(&path)?;
        let wt = WindowTitle::new(cfg.tui().window_title());
        assert_eq!(wt.render(&ctx(Some(&song("s")))), None);
        Ok(())
    }

    /// 默认模板不含 lyric → wants_lyric 为 false（调用方跳过每帧歌词拼接）。
    #[test]
    fn wants_lyric_false_for_default() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let wt = WindowTitle::new(cfg.tui().window_title());
        assert!(!wt.wants_lyric());
        Ok(())
    }

    /// 模板含 lyric 字段 → wants_lyric 为 true。
    #[test]
    fn wants_lyric_true_when_template_references_lyric() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            r#"return { tui = { window_title = { template = { { field = "lyric" } } } } }"#,
        )?;
        let (cfg, _warnings) = mineral_config::load(&path)?;
        let wt = WindowTitle::new(cfg.tui().window_title());
        assert!(wt.wants_lyric());
        Ok(())
    }

    /// 标题里的控制字符（换行 / BEL / ESC）被抹平，杜绝 OSC 注入。
    #[test]
    fn sanitize_strips_control_chars() {
        assert_eq!(sanitize_title("a\x07b\x1bc\nd"), "a b c d");
    }

    /// 连续同上下文只写一次（通过 last_title 观察）。
    #[test]
    fn update_dedupes_same_title() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let mut wt = WindowTitle::new(cfg.tui().window_title());
        let s = with_artist(song("s"), "Artist");
        wt.update(&ctx(Some(&s)))?;
        let first = wt.last_title().map(String::from);
        wt.update(&ctx(Some(&s)))?;
        assert_eq!(wt.last_title(), first.as_deref());
        Ok(())
    }
}
