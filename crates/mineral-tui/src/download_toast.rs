//! 下载这个**通知使用方**:把 [`DownloadProgress`] 翻译进通知层 + 下载专属内容实现
//! (进度条 [`DownloadItem`]、完成提示 [`CompleteItem`])。
//!
//! [`DownloadNotifier`] 持有下载专属去重状态(`last_result_seq`),每帧 [`DownloadNotifier::feed`]
//! 把进度喂成 `set_live(LiveSlot::DOWNLOAD, …)`、把一批下载的成败翻译成一条完成 `flash`。
//! 通知层完全不感知「下载」—— 各内容实现作为 [`ToastItem`] 自己决定怎么画(进度条按进度在红→绿
//! 之间渐变、用 1/8 子格 [`crate::cells::left_eighth`] 平滑填充)。

use mineral_protocol::DownloadProgress;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use ratatui::widgets::Paragraph;

use crate::cells::left_eighth;
use crate::color::lerp_color;
use crate::notifications::{LiveSlot, Notifications};
use crate::theme::Theme;
use crate::toast::ToastItem;

/// 下载 → 通知层的翻译器,持有下载专属去重状态。
pub(crate) struct DownloadNotifier {
    /// 已消费到的下载完成计数;`DownloadProgress.result_seq` 增长一次 → 自发一条完成 flash。
    last_result_seq: u64,
}

impl DownloadNotifier {
    /// 新建翻译器。
    pub(crate) fn new() -> Self {
        Self { last_result_seq: 0 }
    }

    /// 每帧:把当前下载进度喂进通知层。
    ///
    /// - `dp.active` → `set_live(LiveSlot::DOWNLOAD, Some(进度条))`,否则置 `None`(退场);
    /// - `dp.result_seq` 增长且有成败 → 推一条完成 `flash`。
    ///
    /// # Params:
    ///   - `n`: 通知层
    ///   - `dp`: 本帧拉到的下载进度
    pub(crate) fn feed(&mut self, n: &mut Notifications, dp: &DownloadProgress) {
        if dp.result_seq != self.last_result_seq {
            self.last_result_seq = dp.result_seq;
            if dp.last_ok + dp.last_skip + dp.last_fail > 0 {
                n.flash(complete(dp.last_ok, dp.last_skip, dp.last_fail));
            }
        }
        n.set_live(LiveSlot::DOWNLOAD, dp.active.then(|| download(dp.clone())));
    }
}

/// 进度条字符宽。
const BAR_CELLS: u16 = 12;

/// 下载进度做的 toast 内容:`[████▌░░░ 62% 2.4MB/s 3/12]`,进度条按进度变色。
pub(crate) struct DownloadItem {
    /// 进度快照。
    progress: DownloadProgress,
}

/// 用一份下载进度构造 toast 内容(boxed,交给 [`crate::notifications::Notifications::set_live`])。
///
/// # Params:
///   - `progress`: 下载进度快照
///
/// # Return:
///   boxed [`ToastItem`]。
fn download(progress: DownloadProgress) -> Box<dyn ToastItem> {
    Box::new(DownloadItem { progress })
}

impl DownloadItem {
    /// 完成百分比(0..=100)。
    fn pct(&self) -> u64 {
        self.progress
            .bytes_done
            .saturating_mul(100)
            .checked_div(self.progress.bytes_total)
            .unwrap_or(0)
            .min(100)
    }

    /// 进度条右侧的文字后缀:` 62% 2.4MB/s 2/24`(`done/total`,前置一空格与进度条分隔)。
    fn suffix(&self) -> String {
        let p = &self.progress;
        format!(
            " {}% {} {}/{}",
            self.pct(),
            fmt_speed(p.speed_bps),
            p.done,
            p.total
        )
    }
}

impl ToastItem for DownloadItem {
    fn width(&self) -> u16 {
        let suffix = u16::try_from(UnicodeWidthStr::width(self.suffix().as_str())).unwrap_or(0);
        BAR_CELLS.saturating_add(suffix)
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let pct = self.pct();
        // 进度条按进度在 红 → 绿 之间渐变。
        let fill = lerp_color(theme.red, theme.green, pct, 100);
        // 1/8 子格精度:总覆盖 eighths,落到整格 + 一个残格。
        let eighths = pct.saturating_mul(u64::from(BAR_CELLS)).saturating_mul(8) / 100;
        let full = usize::try_from(eighths / 8)
            .unwrap_or(0)
            .min(usize::from(BAR_CELLS));
        let rem = u32::try_from(eighths % 8).unwrap_or(0);
        let mut spans = Vec::<Span<'static>>::new();
        spans.push(Span::styled("█".repeat(full), Style::new().fg(fill)));
        let mut used = full;
        if rem > 0 && used < usize::from(BAR_CELLS) {
            spans.push(Span::styled(
                left_eighth(rem).to_owned(),
                Style::new().fg(fill).bg(theme.overlay),
            ));
            used += 1;
        }
        let track = usize::from(BAR_CELLS).saturating_sub(used);
        if track > 0 {
            spans.push(Span::styled(
                "░".repeat(track),
                Style::new().fg(theme.overlay),
            ));
        }
        spans.push(Span::raw(self.suffix()).fg(theme.subtext));
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.base)),
            area,
        );
    }
}

/// 下载完成提示 toast 内容:`✓N`(绿,真下载)+ `⊙N`(黄,已存在跳过)+ `✗M`(红,失败),
/// **只显示非零部分**。
pub(crate) struct CompleteItem {
    /// 真正下载成功首数。
    ok: usize,

    /// 已存在跳过首数。
    skip: usize,

    /// 失败首数。
    fail: usize,
}

/// 用一批下载的成败 / 跳过数构造完成提示 toast 内容(boxed)。
///
/// # Params:
///   - `ok`: 真正下载成功首数
///   - `skip`: 已存在跳过首数
///   - `fail`: 失败首数
///
/// # Return:
///   boxed [`ToastItem`]。
fn complete(ok: usize, skip: usize, fail: usize) -> Box<dyn ToastItem> {
    Box::new(CompleteItem { ok, skip, fail })
}

impl CompleteItem {
    /// 文字前缀:有真下载 → `下载完成`;否则有失败 → `下载失败`;否则(全已存在)→ `已下载`。
    fn prefix(&self) -> &'static str {
        if self.ok > 0 {
            "下载完成"
        } else if self.fail > 0 {
            "下载失败"
        } else {
            "已下载"
        }
    }

    /// 纯文本形态(量宽用),如 `下载完成 ✓3 ⊙2 ✗1` / `已下载 ⊙2`。
    fn label(&self) -> String {
        let mut parts = vec![self.prefix().to_owned()];
        if self.ok > 0 {
            parts.push(format!("✓{}", self.ok));
        }
        if self.skip > 0 {
            parts.push(format!("⊙{}", self.skip));
        }
        if self.fail > 0 {
            parts.push(format!("✗{}", self.fail));
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
        if self.ok > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("✓{}", self.ok),
                Style::new().fg(theme.green),
            ));
        }
        if self.skip > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("⊙{}", self.skip),
                Style::new().fg(theme.yellow),
            ));
        }
        if self.fail > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("✗{}", self.fail),
                Style::new().fg(theme.red),
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
    use mineral_protocol::DownloadProgress;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::{complete, download, fmt_speed};
    use crate::theme::Theme;
    use crate::toast::{Toast, ToastItem};

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

    /// 下载进度条:展开到位,topbar 一行 `[ 进度条 % 速度 done/total ]`(聚合计数,如 2/24)。
    #[test]
    fn download_bar_snapshot() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let dp = DownloadProgress {
            active: true,
            done: 2,
            total: 24,
            bytes_done: 62,
            bytes_total: 100,
            speed_bps: 2_516_582,
            ..DownloadProgress::default()
        };
        let mut toast = Toast::new();
        expand(&mut toast, || download(dp.clone()), 8);

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            toast.render(f, area, &theme);
        })?;
        crate::test_support::assert_snap!(
            "下载进度条 toast:展开到位,[进度条 % 速度 done/total](聚合计数 2/24)",
            t.backend()
        );
        Ok(())
    }

    /// 下载完成提示三态:`下载完成 ✓3 ⊙2 ✗1`,只显示非零部分(布局)。
    /// 颜色另见 [`complete_colors_ok_green_skip_yellow_fail_red`]。
    #[test]
    fn complete_snapshot() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut toast = Toast::new();
        expand(&mut toast, || complete(3, 2, 1), 8);

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            toast.render(f, area, &theme);
        })?;
        crate::test_support::assert_snap!("下载完成提示:✓3 ⊙2 ✗1(真下载/已存在/失败)", t.backend());
        Ok(())
    }

    /// 完成提示只有成功(skip=fail=0)→ `下载完成 ✓5`。
    #[test]
    fn complete_snapshot_ok_only() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut toast = Toast::new();
        expand(&mut toast, || complete(5, 0, 0), 8);

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            toast.render(f, area, &theme);
        })?;
        crate::test_support::assert_snap!("下载完成提示:全成功 → 只 ✓5", t.backend());
        Ok(())
    }

    /// 整批都已存在(ok=fail=0)→ 前缀变 `已下载`,只显示 `⊙N`。
    #[test]
    fn complete_snapshot_all_skipped() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut toast = Toast::new();
        expand(&mut toast, || complete(0, 2, 0), 8);

        let mut t = Terminal::new(TestBackend::new(60, 3))?;
        t.draw(|f| {
            let area = f.area();
            toast.render(f, area, &theme);
        })?;
        crate::test_support::assert_snap!("下载完成提示:整批已存在 → 已下载 ⊙2", t.backend());
        Ok(())
    }

    /// 颜色断言(snapshot 抓不到色):✓ 那格 fg = 绿、⊙ = 黄、✗ = 红。
    #[test]
    fn complete_colors_ok_green_skip_yellow_fail_red() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let item = complete(3, 2, 1);
        let mut t = Terminal::new(TestBackend::new(30, 1))?;
        t.draw(|f| {
            let area = f.area();
            item.render(f, area, &theme);
        })?;
        let buf = t.backend().buffer();
        // 行形如 " 下载完成 ✓3 ⊙2 ✗1 ":找 ✓ / ⊙ / ✗ 所在 cell,核对 fg。
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
