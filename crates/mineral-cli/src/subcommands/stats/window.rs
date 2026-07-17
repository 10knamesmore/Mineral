//! `stats` 子命令的时间窗与输出格式(§8.2 横切约定)。
//!
//! 时间窗三式互斥([`Window`]);epoch 边界按 **UTC**——与 stats.db 聚合的 UTC 日口径
//! (streak / strftime 分桶)一致,避免报告窗与内部日界错位。输出格式三选一([`Format`])。

use std::ops::Range;

use clap::{Args, ValueEnum};
use color_eyre::eyre::{WrapErr as _, bail};

/// 一天的毫秒数(窗口端点折算用)。
const DAY_MS: i64 = 86_400_000;

/// 报告时间窗(三式互斥;缺省语义由子命令经 [`WindowDefault`] 定)。
#[derive(Args, Debug, Clone)]
#[group(multiple = false)]
pub struct Window {
    /// 某一年(如 2026):该年 1 月 1 日至次年 1 月 1 日(UTC)。
    #[arg(long)]
    year: Option<i32>,

    /// 起始日 YYYY-MM-DD(含),须与 `--to` 成对。
    #[arg(long, requires = "to")]
    from: Option<String>,

    /// 结束日 YYYY-MM-DD(含),须与 `--from` 成对。
    #[arg(long, requires = "from")]
    to: Option<String>,

    /// 全量窗口(不设时间边界)。
    #[arg(long)]
    all: bool,
}

/// 子命令未显式指定任何窗口式时的缺省。
#[derive(Clone, Copy)]
pub enum WindowDefault {
    /// 当前年(`report`)。
    CurrentYear,

    /// 全量(`top` / `history`)。
    All,
}

/// 输出格式
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 默认
    Text,

    /// json
    Json,

    /// markdown
    Md,
}

/// 榜单排序口径(CLI `--by`);映射到 [`mineral_stats::TopBy`]。
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// 按播放次数。
    Plays,

    /// 按收听时长。
    Time,
}

impl From<By> for mineral_stats::TopBy {
    fn from(by: By) -> Self {
        match by {
            By::Plays => Self::Plays,
            By::Time => Self::Time,
        }
    }
}

impl Window {
    /// 解析成 `[start_ms, end_ms)`(UTC)。三式都未给时按 `default` 回落。
    ///
    /// # Params:
    ///   - `default`: 未指定任何窗口式时的缺省
    ///   - `now_ms`: 当前 epoch ms(算「当前年」用)
    ///
    /// # Return:
    ///   epoch ms 半开区间
    pub fn range(&self, default: WindowDefault, now_ms: i64) -> color_eyre::Result<Range<i64>> {
        if let Some(year) = self.year {
            return year_range(year);
        }
        if let (Some(from), Some(to)) = (&self.from, &self.to) {
            let start = day_start_ms(from).wrap_err("--from 日期无效")?;
            let to_start = day_start_ms(to).wrap_err("--to 日期无效")?;
            // 反向窗口(from 晚于 to)会产出空区间、报告静默全空;显式报错而非 swap。
            if start > to_start {
                bail!("--from 晚于 --to({from} > {to}):时间窗为空");
            }
            // to 含当天,故 end 取 to 次日零点(半开区间上界)。
            return Ok(start..to_start.saturating_add(DAY_MS));
        }
        if self.all {
            return Ok(0..i64::MAX);
        }
        match default {
            WindowDefault::CurrentYear => year_range(current_year(now_ms)?),
            WindowDefault::All => Ok(0..i64::MAX),
        }
    }

    /// 供渲染头部的窗口标签(`"2026"` / `"2026-01-01 → 2026-06-30"` / `"all"`)。
    ///
    /// # Params:
    ///   - `default`: 未指定任何窗口式时的缺省
    ///   - `now_ms`: 当前 epoch ms
    ///
    /// # Return:
    ///   人读窗口标签
    pub fn label(&self, default: WindowDefault, now_ms: i64) -> color_eyre::Result<String> {
        if let Some(year) = self.year {
            return Ok(year.to_string());
        }
        if let (Some(from), Some(to)) = (&self.from, &self.to) {
            return Ok(format!("{from} → {to}"));
        }
        if self.all {
            return Ok("all".to_owned());
        }
        match default {
            WindowDefault::CurrentYear => Ok(current_year(now_ms)?.to_string()),
            WindowDefault::All => Ok("all".to_owned()),
        }
    }
}

/// 某年的 `[Jan 1, 次年 Jan 1)` epoch ms(UTC)。
fn year_range(year: i32) -> color_eyre::Result<Range<i64>> {
    let start = calendar_ms(year, 1, 1)?;
    let end = calendar_ms(year.saturating_add(1), 1, 1)?;
    Ok(start..end)
}

/// 当前 epoch ms 落在哪个 UTC 年。
fn current_year(now_ms: i64) -> color_eyre::Result<i32> {
    Ok(time::OffsetDateTime::from_unix_timestamp(now_ms / 1000)
        .wrap_err("当前时间换算失败")?
        .year())
}

/// `"YYYY-MM-DD"` → 当日零点 epoch ms(UTC);格式 / 取值非法报错。供 `--from`/`--to` 与
/// `prune --before` 共用。
pub fn day_start_ms(ymd: &str) -> color_eyre::Result<i64> {
    let mut parts = ymd.split('-');
    let year = parts.next().and_then(|s| s.parse::<i32>().ok());
    let month = parts.next().and_then(|s| s.parse::<u8>().ok());
    let day = parts.next().and_then(|s| s.parse::<u8>().ok());
    match (year, month, day, parts.next()) {
        (Some(y), Some(m), Some(d), None) => calendar_ms(y, m, d),
        _ => bail!("日期须为 YYYY-MM-DD:{ymd:?}"),
    }
}

/// `(year, month 1-12, day)` → 当日零点 epoch ms(UTC)。
fn calendar_ms(year: i32, month: u8, day: u8) -> color_eyre::Result<i64> {
    let month = time::Month::try_from(month).wrap_err_with(|| format!("月份非法:{month}"))?;
    let date = time::Date::from_calendar_date(year, month, day)
        .wrap_err_with(|| format!("日期非法:{year}-{month:?}-{day}"))?;
    date.midnight()
        .assume_utc()
        .unix_timestamp()
        .checked_mul(1000)
        .ok_or_else(|| color_eyre::eyre::eyre!("时间戳溢出 i64"))
}

#[cfg(test)]
mod tests {
    use super::{Format, Window, WindowDefault, calendar_ms, day_start_ms, year_range};
    use clap::ValueEnum as _;

    /// 2026-01-01 00:00:00 UTC 的 epoch ms(锚点,手算校验)。
    const Y2026_START: i64 = 1_767_225_600_000;

    #[test]
    fn calendar_and_year_boundaries_utc() -> color_eyre::Result<()> {
        assert_eq!(calendar_ms(2026, 1, 1)?, Y2026_START, "2026 元旦零点 UTC");
        let range = year_range(2026)?;
        assert_eq!(range.start, Y2026_START);
        // 2026 非闰年 → 365 天。
        assert_eq!(
            range.end - range.start,
            365 * super::DAY_MS,
            "2026 全年 365 天"
        );
        Ok(())
    }

    #[test]
    fn from_to_is_inclusive_of_end_day() -> color_eyre::Result<()> {
        // 单日窗 [d, d+1):from == to 时应覆盖一整天。
        let start = day_start_ms("2026-03-05")?;
        assert_eq!(start, calendar_ms(2026, 3, 5)?);
        Ok(())
    }

    /// 反向窗口(from 晚于 to)报错,不静默产出空区间;同一天算合法单日窗。
    #[test]
    fn reversed_from_to_rejected() -> color_eyre::Result<()> {
        let reversed = Window {
            year: None,
            from: Some("2026-06-30".to_owned()),
            to: Some("2026-01-01".to_owned()),
            all: false,
        };
        assert!(
            reversed.range(WindowDefault::All, 0).is_err(),
            "from 晚于 to 应报错"
        );

        let same_day = Window {
            year: None,
            from: Some("2026-03-05".to_owned()),
            to: Some("2026-03-05".to_owned()),
            all: false,
        };
        assert!(
            same_day.range(WindowDefault::All, 0).is_ok(),
            "同一天为合法单日窗"
        );
        Ok(())
    }

    #[test]
    fn bad_date_rejected() {
        assert!(day_start_ms("2026-13-01").is_err(), "月份 13 非法");
        assert!(day_start_ms("2026-02-30").is_err(), "2 月 30 日非法");
        assert!(day_start_ms("not-a-date").is_err());
        assert!(day_start_ms("2026-01").is_err(), "缺日");
    }

    /// `--year` 优先于缺省;缺省 All → 全量。
    #[test]
    fn window_resolution_precedence() -> color_eyre::Result<()> {
        let all = Window {
            year: None,
            from: None,
            to: None,
            all: true,
        };
        assert_eq!(all.range(WindowDefault::CurrentYear, 0)?, 0..i64::MAX);

        let yr = Window {
            year: Some(2026),
            from: None,
            to: None,
            all: false,
        };
        assert_eq!(yr.range(WindowDefault::All, 0)?, year_range(2026)?);
        assert_eq!(yr.label(WindowDefault::All, 0)?, "2026");
        Ok(())
    }

    #[test]
    fn format_parses_lowercase() -> color_eyre::Result<()> {
        assert_eq!(
            Format::from_str("json", /*ignore_case*/ true).ok(),
            Some(Format::Json)
        );
        assert_eq!(Format::from_str("md", true).ok(), Some(Format::Md));
        assert_eq!(Format::from_str("text", true).ok(), Some(Format::Text));
        Ok(())
    }
}
