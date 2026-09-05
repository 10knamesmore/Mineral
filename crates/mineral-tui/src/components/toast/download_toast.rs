//! 下载这个通知使用方:把 [`DownloadSummary`] 翻译进通知层 + 下载专属内容实现。
//!
//! [`DownloadNotifier`] 持有 wave sequence 去重状态，每帧把 active/queued summary 喂成 live toast，
//! 把一波下载的结果翻译成一条完成 flash。

use mineral_protocol::{DownloadSummary, DownloadWave};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use ratatui::widgets::Paragraph;

use crate::components::toast::notifications::{LiveSlot, Notifications};
use crate::components::toast::toast::ToastItem;
use crate::render::theme::Theme;

/// 下载 → 通知层的翻译器,持有下载专属去重状态。
pub(crate) struct DownloadNotifier {
    /// 已消费到的 wave sequence；增长一次只发一条完成 flash。
    last_wave_sequence: u64,
}

impl DownloadNotifier {
    /// 新建翻译器。
    pub(crate) fn new() -> Self {
        Self {
            last_wave_sequence: 0,
        }
    }

    /// 每帧:把当前下载进度喂进通知层。
    ///
    /// 有 queued/active/preparing 时保留 live summary；最新 wave sequence 增长时发一条 flash。
    ///
    /// # Params:
    ///   - `n`: 通知层
    ///   - `summary`: 本帧拉到的小型下载汇总
    pub(crate) fn feed(&mut self, n: &mut Notifications, summary: &DownloadSummary) {
        if let Some(wave) = &summary.latest_wave
            && wave.sequence != self.last_wave_sequence
        {
            self.last_wave_sequence = wave.sequence;
            n.flash(complete(wave.clone()));
        }
        let active = summary.active > 0 || summary.queued > 0 || summary.preparing_playlists > 0;
        n.set_live(
            LiveSlot::DOWNLOAD,
            active.then(|| download(summary.clone())),
        );
    }
}

/// Download live toast content.
pub(crate) struct DownloadItem {
    /// Small download summary.
    summary: DownloadSummary,
}

/// 用一份下载进度构造 toast 内容(boxed,交给 [`crate::components::toast::notifications::Notifications::set_live`])。
///
/// # Params:
///   - `summary`: 小型下载汇总
///
/// # Return:
///   boxed [`ToastItem`]。
fn download(summary: DownloadSummary) -> Box<dyn ToastItem> {
    Box::new(DownloadItem { summary })
}

impl DownloadItem {
    /// Human-readable live summary.
    fn label(&self) -> String {
        let summary = &self.summary;
        let mut parts = Vec::<String>::new();
        if summary.active > 0 {
            parts.push(format!("↓ {} songs", summary.active));
        }
        if summary.queued > 0 {
            parts.push(format!("{} queued", summary.queued));
        }
        if summary.preparing_playlists > 0 {
            parts.push(format!(
                "preparing {} playlist",
                summary.preparing_playlists
            ));
        }
        if summary.speed_bps > 0 {
            parts.push(fmt_speed(summary.speed_bps));
        }
        parts.push("D details".to_owned());
        parts.join(" · ")
    }
}

impl ToastItem for DownloadItem {
    fn width(&self) -> u16 {
        u16::try_from(UnicodeWidthStr::width(self.label().as_str())).unwrap_or(0)
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        frame.render_widget(
            Paragraph::new(Line::from(Span::raw(self.label()).fg(theme.subtext)))
                .style(Style::new().bg(theme.base)),
            area,
        );
    }
}

/// Settled-wave toast content. Downloaded, already-present, hook-skipped, failed, and stopped
/// segments are shown only when nonzero.
pub(crate) struct CompleteItem {
    /// Settled wave counts.
    wave: DownloadWave,
}

/// 用一批下载的成败 / 跳过数构造完成提示 toast 内容(boxed)。
///
/// # Params:
///   - `wave`: Settled wave counts.
///
/// # Return:
///   boxed [`ToastItem`]。
fn complete(wave: DownloadWave) -> Box<dyn ToastItem> {
    Box::new(CompleteItem { wave })
}

impl CompleteItem {
    /// 文字前缀:有真下载 → `下载完成`;否则有失败 → `下载失败`;否则(全已存在)→ `已下载`。
    fn prefix(&self) -> &'static str {
        "Downloads finished"
    }

    /// Plain-text form used for width measurement.
    fn label(&self) -> String {
        let mut parts = vec![self.prefix().to_owned()];
        if self.wave.downloaded > 0 {
            parts.push(format!("✓{}", self.wave.downloaded));
        }
        if self.wave.already_present > 0 {
            parts.push(format!("⊙ already {}", self.wave.already_present));
        }
        if self.wave.skipped_by_hook > 0 {
            parts.push(format!("⊘ skipped {}", self.wave.skipped_by_hook));
        }
        if self.wave.failed > 0 {
            parts.push(format!("✗ failed {}", self.wave.failed));
        }
        if self.wave.stopped > 0 {
            parts.push(format!("■ stopped {}", self.wave.stopped));
        }
        parts.join(" ")
    }
}

impl ToastItem for CompleteItem {
    fn width(&self) -> u16 {
        u16::try_from(UnicodeWidthStr::width(self.label().as_str()))
            .unwrap_or(0)
            .saturating_add(2) // 左右各留一空格
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        // 与 `label()` 逐段对应:文字前缀(中性色)+ 彩色 ✓(绿)/⊙(黄)/✗(红) 计数,前后各留一空格。
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(self.prefix(), Style::new().fg(theme.subtext)),
        ];
        if self.wave.downloaded > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("✓{}", self.wave.downloaded),
                Style::new().fg(theme.green),
            ));
        }
        if self.wave.already_present > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("⊙ already {}", self.wave.already_present),
                Style::new().fg(theme.yellow),
            ));
        }
        if self.wave.skipped_by_hook > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("⊘ skipped {}", self.wave.skipped_by_hook),
                Style::new().fg(theme.yellow),
            ));
        }
        if self.wave.failed > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("✗ failed {}", self.wave.failed),
                Style::new().fg(theme.red),
            ));
        }
        if self.wave.stopped > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("■ stopped {}", self.wave.stopped),
                Style::new().fg(theme.overlay),
            ));
        }
        spans.push(Span::raw(" "));
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.base)),
            area,
        );
    }
}

/// 速度(字节/秒)→ 人读字符串,整数定点(项目禁 `as` 浮点强转)。
///
/// # Params:
///   - `bps`: 字节/秒
///
/// # Return:
///   如 `2.4MB/s` / `512KB/s` / `30B/s`。
fn fmt_speed(bps: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bps >= MB {
        let tenths = bps.saturating_mul(10) / MB;
        format!("{}.{}MB/s", tenths / 10, tenths % 10)
    } else if bps >= KB {
        format!("{}KB/s", bps / KB)
    } else {
        format!("{bps}B/s")
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{DownloadSummary, DownloadWave};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::{complete, download, fmt_speed};
    use crate::components::toast::toast::{Toast, ToastItem};

    /// Builds settled wave counts for existing toast tests.
    fn wave(downloaded: usize, already_present: usize, failed: usize) -> DownloadWave {
        DownloadWave {
            sequence: 1,
            downloaded,
            already_present,
            skipped_by_hook: 0,
            failed,
            stopped: 0,
        }
    }

    #[test]
    fn speed_units() {
        assert_eq!(fmt_speed(30), "30B/s");
        assert_eq!(fmt_speed(2048), "2KB/s");
        assert_eq!(fmt_speed(2_516_582), "2.3MB/s");
    }

    /// 跑 n tick 推进 toast 动画(每帧重新声明同一内容,模拟持续展开)。
    fn expand(toast: &mut Toast, item: impl Fn() -> Box<dyn ToastItem>, n: usize) {
        for _ in 0..n {
            toast.set(Some(item()));
            toast.tick();
        }
    }

    /// 下载 live summary 展开到位。
    #[test]
    fn download_bar_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let summary = DownloadSummary {
            active: 2,
            queued: 5,
            preparing_playlists: 0,
            speed_bps: 2_516_582,
            latest_wave: None,
        };
        let mut toast = Toast::new(/*anim_ticks*/ 6);
        expand(&mut toast, || download(summary.clone()), 8);

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            toast.render(f, area, &theme, /*blend*/ 0, /*shrink*/ 1000);
        })?;
        crate::test_support::assert_snap!(
            "下载 summary toast:2 active,5 queued,aggregate speed,D details",
            t.backend()
        );
        Ok(())
    }

    /// Settled-wave toast shows each nonzero outcome segment.
    /// 颜色另见 [`complete_colors_ok_green_skip_yellow_fail_red`]。
    #[test]
    fn complete_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut toast = Toast::new(/*anim_ticks*/ 6);
        expand(&mut toast, || complete(wave(3, 2, 1)), 8);

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            toast.render(f, area, &theme, /*blend*/ 0, /*shrink*/ 1000);
        })?;
        crate::test_support::assert_snap!(
            "下载 wave 完成提示:downloaded / already-present / failed",
            t.backend()
        );
        Ok(())
    }

    /// A wave with only committed downloads shows one outcome segment.
    #[test]
    fn complete_snapshot_ok_only() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut toast = Toast::new(/*anim_ticks*/ 6);
        expand(&mut toast, || complete(wave(5, 0, 0)), 8);

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            toast.render(f, area, &theme, /*blend*/ 0, /*shrink*/ 1000);
        })?;
        crate::test_support::assert_snap!("下载 wave 完成提示:仅 downloaded", t.backend());
        Ok(())
    }

    /// A wave with only existing exports shows one outcome segment.
    #[test]
    fn complete_snapshot_all_skipped() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut toast = Toast::new(/*anim_ticks*/ 6);
        expand(&mut toast, || complete(wave(0, 2, 0)), 8);

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            toast.render(f, area, &theme, /*blend*/ 0, /*shrink*/ 1000);
        })?;
        crate::test_support::assert_snap!("下载 wave 完成提示:仅 already-present", t.backend());
        Ok(())
    }

    /// 颜色断言(snapshot 抓不到色):✓ 那格 fg = 绿、⊙ = 黄、✗ = 红。
    #[test]
    fn complete_colors_ok_green_skip_yellow_fail_red() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let item = complete(wave(3, 2, 1));
        let mut t = Terminal::new(TestBackend::new(80, 1))?;
        t.draw(|f| {
            let area = f.area();
            item.render(f, area, &theme);
        })?;
        let buf = t.backend().buffer();
        // Locate the outcome glyph cells and verify their semantic colors.
        let mut green = false;
        let mut yellow = false;
        let mut red = false;
        for x in 0..buf.area.width {
            let Some(cell) = buf.cell((x, 0)) else {
                continue;
            };
            match cell.symbol() {
                "✓" => green = cell.fg == theme.green,
                "⊙" => yellow = cell.fg == theme.yellow,
                "✗" => red = cell.fg == theme.red,
                _ => {}
            }
        }
        assert!(green, "✓ 应为绿色 {:?}", theme.green);
        assert!(yellow, "⊙ 应为黄色 {:?}", theme.yellow);
        assert!(red, "✗ 应为红色 {:?}", theme.red);
        Ok(())
    }
}
