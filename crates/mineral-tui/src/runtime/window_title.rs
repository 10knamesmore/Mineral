//! TUI 窗口标题：把当前播放态实时写进终端任务栏 / tab 标题。
//!
//! 标题是既有 client 状态的纯派生，按优先级取：断连（client 强制）> 脚本旋钮
//! 覆盖 > 结构化四态模板（播放 / 暂停 / 空闲各一套，`StateIcon` 段按当前态解析
//! 图标字形）。写入走 crossterm `SetTitle` + 变化检测，避免每帧刷 OSC。

use std::io;

use crossterm::execute;
use crossterm::terminal::SetTitle;
use mineral_config::{TimeFormat, TitleField, TitleIcons, TitleSegment, WindowTitleConfig};
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

    /// 当前曲目全长（ms；0 = 未探出）。
    pub duration_ms: u64,

    /// 当前歌词行文本，已按 [`sync_trust`](crate::runtime::playback::Playback::sync_trust)
    /// 过滤（失真档为 `None`）。
    pub lyric: Option<&'a str>,

    /// `window_title.text` 旋钮当前值（脚本自渲染整串；`None` = 无覆盖）。
    pub override_text: Option<&'a str>,
}

/// 窗口标题的状态机 + 变化检测。
pub(crate) struct WindowTitle {
    /// 总开关。
    enabled: bool,

    /// 四态状态图标字形。
    icons: TitleIcons,

    /// 有歌态（播放 / 暂停共用）模板。
    template: Vec<TitleSegment>,

    /// 空闲态模板。
    idle: Vec<TitleSegment>,

    /// 断连态模板。
    disconnected: Vec<TitleSegment>,

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
        Self {
            enabled: *cfg.enabled(),
            icons: cfg.icons().clone(),
            template: cfg.template().clone(),
            idle: cfg.idle().clone(),
            disconnected: cfg.disconnected().clone(),
            last_title: None,
        }
    }

    /// 根据当前上下文按优先级渲染标题字符串。
    ///
    /// # Params:
    ///   - `ctx`: 当帧上下文。
    ///
    /// # Return:
    ///   应显示的新标题；禁用时返回 `None`。
    fn render(&self, ctx: &TitleContext<'_>) -> Option<String> {
        if !self.enabled {
            return None;
        }
        // 断连优先且 client 强制：此时脚本通道已死，旋钮值 stale，忽略。
        if !ctx.connected {
            return Some(fold(&self.disconnected, self.icons.disconnected(), ctx));
        }
        // 旋钮覆盖胜过结构化模板（脚本自渲染整串）。
        if let Some(text) = ctx.override_text {
            return Some(text.to_owned());
        }
        let Some(_song) = ctx.song else {
            return Some(fold(&self.idle, self.icons.idle(), ctx));
        };
        let icon = if ctx.playing {
            self.icons.playing()
        } else {
            self.icons.paused()
        };
        Some(fold(&self.template, icon, ctx))
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
            execute!(io::stdout(), SetTitle(title))?;
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

/// fold 一套模板：`StateIcon` → 当前态字形 + 空格；`Field` → 按字段取值，空值折叠
/// （连同 prefix/suffix）；`Literal` 原样。
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
            TitleSegment::StateIcon { .. } => {
                out.push_str(icon);
                out.push(' ');
            }
            TitleSegment::Field {
                field,
                prefix,
                suffix,
                format,
            } => {
                if let Some(v) = field_value(*field, format, ctx)
                    && !v.is_empty()
                {
                    out.push_str(prefix);
                    out.push_str(&v);
                    out.push_str(suffix);
                }
            }
            TitleSegment::Literal { text } => out.push_str(text),
        }
    }
    out
}

/// 取一个字段的渲染值（`None` = 该段折叠）。
///
/// `Position` 恒渲染（0 → `00:00`）；`Duration` 为 0（未探出）折叠；歌曲派生字段
/// 无歌 / 无值折叠；`Lyric` 无当前行折叠。
fn field_value(field: TitleField, format: &TimeFormat, ctx: &TitleContext<'_>) -> Option<String> {
    match field {
        TitleField::Title => ctx.song.map(|s| s.name.clone()),
        TitleField::Artist => ctx
            .song
            .and_then(|s| s.artists.first())
            .map(|a| a.name.clone()),
        TitleField::Album => ctx
            .song
            .and_then(|s| s.album.as_ref())
            .map(|a| a.name.clone()),
        TitleField::Source => ctx.song.map(|s| s.source().label().to_owned()),
        TitleField::Position => Some(format.render(ctx.position_ms)),
        TitleField::Duration => (ctx.duration_ms > 0).then(|| format.render(ctx.duration_ms)),
        TitleField::Lyric => ctx.lyric.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use mineral_config::{Config, TimeFormat, TimePreset, TitleField, TitleSegment};
    use mineral_model::Song;
    use mineral_test::{song, with_album, with_artist};

    use super::{TitleContext, WindowTitle};

    /// 造一个默认上下文（播放中、已连接、无进度 / 歌词 / 覆盖）。
    fn ctx(song: Option<&Song>) -> TitleContext<'_> {
        TitleContext {
            song,
            playing: true,
            connected: true,
            position_ms: 0,
            duration_ms: 0,
            lyric: None,
            override_text: None,
        }
    }

    /// 用给定 template 造一个 WindowTitle（idle/disconnected 留空，图标取 default.lua）。
    fn wt(template: Vec<TitleSegment>) -> color_eyre::Result<WindowTitle> {
        let cfg = Config::defaults()?;
        Ok(WindowTitle {
            enabled: true,
            icons: cfg.tui().window_title().icons().clone(),
            template,
            idle: Vec::new(),
            disconnected: Vec::new(),
            last_title: None,
        })
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

    /// position / duration 字段：clock 预设，duration=0 折叠、position=0 渲染 00:00。
    #[test]
    fn position_and_duration_clock() -> color_eyre::Result<()> {
        let wt = wt(Vec::from([
            field(TitleField::Position, "", ""),
            field(TitleField::Duration, "/", ""),
        ]))?;
        let s = song("s");
        // duration=0 → 折叠；position=0 → 00:00。
        let mut c = ctx(Some(&s));
        assert_eq!(wt.render(&c), Some("00:00".to_owned()));
        // 有进度 + 全长。
        c.position_ms = 83_000;
        c.duration_ms = 296_000;
        assert_eq!(wt.render(&c), Some("01:23/04:56".to_owned()));
        Ok(())
    }

    /// source 字段渲染来源 label。
    #[test]
    fn source_field() -> color_eyre::Result<()> {
        let wt = wt(Vec::from([field(TitleField::Source, "", "")]))?;
        let s = song("s"); // mineral_test::song → netease 命名空间
        assert_eq!(
            wt.render(&ctx(Some(&s))),
            Some(s.source().label().to_owned())
        );
        Ok(())
    }

    /// lyric 字段：有当前行则渲染，无则折叠。
    #[test]
    fn lyric_field_renders_and_folds() -> color_eyre::Result<()> {
        let wt = wt(Vec::from([field(TitleField::Lyric, "♪ ", "")]))?;
        let s = song("s");
        let mut c = ctx(Some(&s));
        c.lyric = Some("这是当前歌词");
        assert_eq!(wt.render(&c), Some("♪ 这是当前歌词".to_owned()));
        c.lyric = None;
        assert_eq!(wt.render(&c), Some(String::new()), "无当前行整段折叠");
        Ok(())
    }

    /// 自定义时间格式串（pattern）。
    #[test]
    fn custom_pattern_format() -> color_eyre::Result<()> {
        let wt = wt(Vec::from([TitleSegment::Field {
            field: TitleField::Position,
            prefix: String::new(),
            suffix: String::new(),
            format: TimeFormat::Pattern {
                pattern: "{m}分{ss}秒".to_owned(),
            },
        }]))?;
        let s = song("s");
        let mut c = ctx(Some(&s));
        c.position_ms = 83_000;
        assert_eq!(wt.render(&c), Some("1分23秒".to_owned()));
        Ok(())
    }

    /// album 字段与 seconds 预设。
    #[test]
    fn album_and_seconds() -> color_eyre::Result<()> {
        let wt = wt(Vec::from([
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
        ]))?;
        let s = with_album(song("s"), "My Album");
        let mut c = ctx(Some(&s));
        c.position_ms = 42_000;
        assert_eq!(wt.render(&c), Some("[My Album] 42s".to_owned()));
        Ok(())
    }

    /// 禁用时不产生任何标题。
    #[test]
    fn disabled_returns_none() -> color_eyre::Result<()> {
        let mut wt = wt(Vec::new())?;
        wt.enabled = false;
        assert_eq!(wt.render(&ctx(Some(&song("s")))), None);
        Ok(())
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
