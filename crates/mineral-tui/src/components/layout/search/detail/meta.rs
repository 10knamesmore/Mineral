//! 实体详情头部的元数据呈现：选择名称、艺人、计量与简介文本，绘制固定头部和独立滚动的简介视口。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};

use mineral_model::{Album, Artist};

use crate::components::layout::search::panel::join_artists;
use crate::render::theme::Theme;
use crate::runtime::state::{DetailData, DetailFrame, EntityRef};

use super::description;

/// 元数据区：上半固定 header（名 + 次行 + 计量，wrap 折行），下半是独立可滚动简介视口
/// （C-d/u/b/f 滚动，按 `\n` 多行渲染、溢出画滚动条）。两者间留一行视觉间隔。
pub(super) fn draw_meta(
    buf: &mut Buffer,
    meta_a: Rect,
    dframe: &DetailFrame,
    theme: &Theme,
    show_back: bool,
) {
    let pad = Rect::new(
        meta_a.x.saturating_add(1),
        meta_a.y,
        meta_a.width.saturating_sub(1),
        meta_a.height,
    );
    if pad.width == 0 || pad.height == 0 {
        return;
    }
    let mut header = meta_lines(dframe, theme);
    if show_back {
        header.push(Line::from(Span::styled(
            "‹ Esc back",
            Style::new().fg(theme.peach),
        )));
    }
    // header 占其行数 + 1 行间隔；简介拿剩余高度（不足时 Layout 自动裁）。
    let head_h = u16::try_from(header.len()).unwrap_or(0).saturating_add(1);
    let [head_a, desc_a] =
        Layout::vertical([Constraint::Length(head_h), Constraint::Min(0)]).areas(pad);
    Widget::render(
        Paragraph::new(header).wrap(Wrap { trim: false }),
        head_a,
        buf,
    );
    description::draw_description(
        buf,
        desc_a,
        frame_description(dframe),
        dframe.description_scroll(),
        theme,
    );
}

/// 当前帧头部该展示的简介原文（歌曲取其所属专辑的、专辑/artist 取聚合 detail 的、歌单取自身的）；
/// 拿不到为空串（不渲染）。
fn frame_description(dframe: &DetailFrame) -> &str {
    match &dframe.entity {
        EntityRef::Song(_) => match &dframe.data {
            Some(DetailData::Album(a)) => &a.description,
            _ => "",
        },
        EntityRef::Album(_) => dframe.album_meta().map_or("", |a| a.description.as_str()),
        EntityRef::Artist(_) => dframe.artist_meta().map_or("", |a| a.description.as_str()),
        EntityRef::Playlist(p) => &p.description,
    }
}

/// 元数据行内容（按实体类型）。artist 帧的计数/简介取 fetch 回来的完整 detail——结果列那份
/// `entity` 来自搜索端点，无 `album_count`/`song_count`/`description`；未到货退回 `entity`。
fn meta_lines(dframe: &DetailFrame, theme: &Theme) -> Vec<Line<'static>> {
    let name = Style::new().fg(theme.text).add_modifier(Modifier::BOLD);
    let sub = Style::new().fg(theme.subtext);
    let dim = Style::new().fg(theme.overlay);
    match &dframe.entity {
        EntityRef::Song(s) => match &dframe.data {
            // 歌曲的详情即其所属专辑:专辑详情到货就照专辑卡片画(名/艺人/计量/简介),与
            // 「直接搜 album」的详情同一套;未到货退回歌名 + 艺人占位（专辑名作标题）。
            Some(DetailData::Album(a)) => album_card_lines(a, theme),
            _ => {
                let title = s
                    .album
                    .as_ref()
                    .map_or_else(|| s.name.clone(), |a| a.name.clone());
                vec![
                    Line::from(Span::styled(title, name)),
                    Line::from(Span::styled(join_artists(&s.artists), sub)),
                ]
            }
        },
        EntityRef::Album(entity_a) => {
            // 整份用 album_meta 选定的 album（fetch 完整 detail 优先、entity 占位兜底）。
            album_card_lines(dframe.album_meta().unwrap_or(&**entity_a), theme)
        }
        EntityRef::Artist(entity_a) => {
            // 整份用 artist_meta 选定的 artist（fetch 完整 detail 优先、entity 占位兜底）；
            // 渲染层只读字段，不关心数据来自哪个端点——聚合已在 channel 边缘完成。
            let a = dframe.artist_meta().unwrap_or(&**entity_a);
            let mut lines = vec![Line::from(Span::styled(a.name.clone(), name))];
            // 粉丝数未知(接口没给)整行省略,不画 `0 fans` 撒谎。
            if let Some(fans) = a.follower_count {
                lines.push(Line::from(Span::styled(
                    format!("{} fans", with_commas(fans)),
                    sub,
                )));
            }
            if let Some(counts) = artist_counts(a) {
                lines.push(Span::styled(counts, dim).into());
            }
            lines
        }
        EntityRef::Playlist(p) => vec![
            Line::from(Span::styled(p.name.clone(), name)),
            Line::from(Span::styled(format!("{} tracks", p.track_count), sub)),
        ],
    }
}

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
    if let Some(count) = a.track_count {
        parts.push(format!("{} tracks", with_commas(count)));
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
