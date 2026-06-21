//! 实体详情头部的元数据文本格式化:专辑卡片行(名 / 艺人 / 计量)、发行年份、艺人计数、
//! 千分位数字。纯文本构造,不碰 Buffer。

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use mineral_model::{Album, Artist};

use crate::components::layout::search_panel::join_artists;
use crate::render::theme::Theme;

/// u64 千分位：`8900000` → `8,900,000`（detail 头部宽，展示完整数而非缩写）。
pub(crate) fn with_commas(n: u64) -> String {
    let s = n.to_string();
    let len = s.chars().count();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 专辑卡片的 header 行:名(bold)+ 艺人 + 计量行(N tracks · 年 · 厂牌)。简介不在此,走独立
/// 可滚动视口。
///
/// album 帧与 song 帧共用同一套——歌曲的详情即其所属专辑,故选中歌时头部与「直接搜 album」
/// 看到的一致。
pub(crate) fn album_card_lines(a: &Album, theme: &Theme) -> Vec<Line<'static>> {
    let name = Style::new().fg(theme.text).add_modifier(Modifier::BOLD);
    let sub = Style::new().fg(theme.subtext);
    let dim = Style::new().fg(theme.overlay);
    let mut lines = vec![
        Line::from(Span::styled(a.name.clone(), name)),
        Line::from(Span::styled(join_artists(&a.artists), sub)),
    ];
    if let Some(meta) = album_meta_line(a) {
        lines.push(Line::from(Span::styled(meta, dim)));
    }
    lines
}

/// album 计量行 `N tracks · 2015 · 厂牌`（缺哪个省哪个；全缺 → `None`）。
fn album_meta_line(a: &Album) -> Option<String> {
    let mut parts = Vec::<String>::new();
    if a.track_count > 0 {
        parts.push(format!("{} tracks", with_commas(a.track_count)));
    }
    if let Some(year) = publish_year(a.publish_time_ms) {
        parts.push(year.to_string());
    }
    if let Some(company) = a.company.as_ref().filter(|c| !c.is_empty()) {
        parts.push(company.clone());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// epoch 毫秒 → 发行年份；`<= 0`（未知）或换算失败 → `None`。
///
/// netease `publishTime` 是**北京零点**对齐的时间戳,故按 +8 偏移读年份——直接用 UTC 会让
/// 北京 1 月 1 日发行的专辑落到上一年 12 月 31 日、年份少算一年。
pub(crate) fn publish_year(ms: i64) -> Option<i32> {
    if ms <= 0 {
        return None;
    }
    let beijing = time::UtcOffset::from_hms(8, 0, 0).ok()?;
    let dt = time::OffsetDateTime::from_unix_timestamp(ms / 1000).ok()?;
    Some(dt.to_offset(beijing).year())
}

/// artist 计数行 `N albums · M songs`；两者皆 `None` → `None`（缺哪个省哪个）。
pub(crate) fn artist_counts(a: &Artist) -> Option<String> {
    let mut parts = Vec::<String>::new();
    if let Some(n) = a.album_count {
        parts.push(format!("{} albums", with_commas(n)));
    }
    if let Some(n) = a.song_count {
        parts.push(format!("{} songs", with_commas(n)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::publish_year;

    /// 发行年份按北京 +8 偏移读：北京 1 月 1 日发行的专辑不能因 UTC 落到上一年。
    #[test]
    fn publish_year_uses_beijing_offset() {
        // 2015-09-26 00:00 北京（= 2015-09-25 16:00 UTC）→ 2015（两边同年,基线）。
        assert_eq!(publish_year(1_443_196_800_000), Some(2015));
        // 2020-01-01 00:00 北京（= 2019-12-31 16:00 UTC）→ 2020；UTC 读会错成 2019。
        assert_eq!(
            publish_year(1_577_808_000_000),
            Some(2020),
            "北京跨年不少算一年"
        );
        // 未知发行时间。
        assert_eq!(publish_year(0), None);
        assert_eq!(publish_year(-1), None);
    }
}
