//! 缓存状态 / 清理报告的渲染:把快照数据转成 comfy-table 文本。
//!
//! 渲染纯函数化(输入全显式给定,含 `now`),便于快照测试。颜色经 `color` 开关控制:
//! `false`(非 tty / 测试)强制无 ANSI,`true`(tty)启用上色。

use std::collections::BTreeMap;
use std::time::SystemTime;

use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use mineral_persist::{CacheStats, PlaylistCacheStats};

/// 占用率进度条格数。
const BAR_WIDTH: u64 = 10;

/// 一条音频缓存项 + 其文件 mtime(供「最旧 / 最新」与逐条清单用)。
pub(super) struct AudioEntry {
    /// 相对路径 `<source>/<quality>/<album>/<title>.<ext>`。
    pub(super) relpath: String,

    /// 文件字节数。
    pub(super) bytes: u64,

    /// 文件修改时间;stat 失败为 `None`。
    pub(super) mtime: Option<SystemTime>,
}

/// 音频缓存的渲染输入。
pub(super) struct AudioInput {
    /// 各缓存项(带 mtime)。
    pub(super) entries: Vec<AudioEntry>,

    /// 总字节。
    pub(super) total_bytes: u64,

    /// 容量上限;`None` = 不驱逐。
    pub(super) capacity: Option<u64>,
}

/// 封面缓存的渲染输入(无需逐条)。
pub(super) struct CoverInput {
    /// 条数。
    pub(super) count: usize,

    /// 总字节。
    pub(super) total_bytes: u64,

    /// 容量上限;`None` = 不驱逐。
    pub(super) capacity: Option<u64>,
}

/// 渲染 `cache status` 报告。
///
/// # Params:
///   - `audio`: 音频缓存输入
///   - `cover`: 封面缓存输入
///   - `playlist`: 歌单缓存计数
///   - `detail`: 是否展示逐条清单与按音质分布
///   - `color`: 是否上色(非 tty 传 `false`)
///   - `now`: 当前时间(算相对时间用,显式传入以便测试)
///
/// # Return:
///   多块表格拼成的报告文本(无尾换行)。
pub(super) fn render_status(
    audio: &AudioInput,
    cover: &CoverInput,
    playlist: &PlaylistCacheStats,
    detail: bool,
    color: bool,
    now: SystemTime,
) -> String {
    let mut blocks = vec![summary_table(audio, cover, playlist, color)];
    if !audio.entries.is_empty() {
        blocks.push(labeled("分格式", &format_table(&audio.entries, color)));
        if let Some(table) = extremes_table(&audio.entries, now, color) {
            blocks.push(labeled("最旧 / 最新", &table));
        }
        if detail {
            blocks.push(labeled("按音质", &quality_table(&audio.entries, color)));
            blocks.push(labeled("逐条", &detail_table(&audio.entries, now, color)));
        }
    } else {
        blocks.push("（音频缓存为空）".to_owned());
    }
    blocks.join("\n")
}

/// 渲染 `cache clean` 报告(各区域清理前 → 已清空 + 音频分格式 + 总释放)。
///
/// # Params:
///   - `audio`: 音频缓存清理回执
///   - `cover`: 封面缓存清理回执
///   - `playlist`: 歌单缓存清理回执
///   - `color`: 是否上色
///
/// # Return:
///   报告文本(无尾换行)。
pub(super) fn render_clean(
    audio: &CacheStats,
    cover: &CacheStats,
    playlist: &PlaylistCacheStats,
    color: bool,
) -> String {
    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("区域", color),
        head_cell("清理前", color),
        head_cell("之后", color),
    ]);
    table.add_row(vec![
        label_cell("音频", color),
        Cell::new(format!(
            "{} 条 / {}",
            audio.entries.len(),
            human_bytes(audio.total_bytes)
        )),
        cleared_cell(color),
    ]);
    table.add_row(vec![
        label_cell("封面", color),
        Cell::new(format!(
            "{} 条 / {}",
            cover.entries.len(),
            human_bytes(cover.total_bytes)
        )),
        cleared_cell(color),
    ]);
    table.add_row(vec![
        label_cell("歌单", color),
        Cell::new(format!(
            "{} 个歌单 / {} 曲目",
            playlist.playlists, playlist.tracks
        )),
        cleared_cell(color),
    ]);

    let mut blocks = vec![table.to_string()];
    if !audio.entries.is_empty() {
        let entries = audio
            .entries
            .iter()
            .map(|e| AudioEntry {
                relpath: e.relpath.clone(),
                bytes: e.bytes,
                mtime: None,
            })
            .collect::<Vec<_>>();
        blocks.push(labeled("音频分格式", &format_table(&entries, color)));
    }
    let freed = audio.total_bytes.saturating_add(cover.total_bytes);
    blocks.push(format!("共释放 {}", human_bytes(freed)));
    blocks.join("\n")
}

/// 汇总表:音频 / 封面 / 歌单各一行。
fn summary_table(
    audio: &AudioInput,
    cover: &CoverInput,
    playlist: &PlaylistCacheStats,
    color: bool,
) -> String {
    let mut table = base_table(color);
    table.set_header(vec![head_cell("缓存", color), head_cell("状态", color)]);
    table.add_row(vec![
        label_cell("音频", color),
        usage_cell(
            audio.total_bytes,
            audio.capacity,
            audio.entries.len(),
            color,
        ),
    ]);
    table.add_row(vec![
        label_cell("封面", color),
        usage_cell(cover.total_bytes, cover.capacity, cover.count, color),
    ]);
    table.add_row(vec![
        label_cell("歌单", color),
        Cell::new(format!(
            "{} 个歌单 · {} 曲目",
            playlist.playlists, playlist.tracks
        )),
    ]);
    table.to_string()
}

/// 「已用 / 上限 占用条 占用率 · N 条」单元格,按占用率上色。
fn usage_cell(total: u64, capacity: Option<u64>, count: usize, color: bool) -> Cell {
    let used = human_bytes(total);
    match occupancy(total, capacity) {
        Some((pct, bar)) => {
            let cap = human_bytes(capacity.unwrap_or(0));
            let cell = Cell::new(format!("{used} / {cap}  {bar} {pct}%  · {count} 条"));
            maybe_fg(cell, color, level_color(pct))
        }
        None => maybe_fg(
            Cell::new(format!("{used}  · {count} 条")),
            color,
            Color::Green,
        ),
    }
}

/// 分格式表:按扩展名聚合(条数 + 大小)。
fn format_table(entries: &[AudioEntry], color: bool) -> String {
    group_table(entries, color, "格式", |rel| audio_ext(rel).to_owned())
}

/// 按音质表:按 relpath 第 2 段聚合。
fn quality_table(entries: &[AudioEntry], color: bool) -> String {
    group_table(entries, color, "音质", |rel| {
        audio_quality(rel).to_owned()
    })
}

/// 通用聚合表:`key` 把 relpath 映射到分组名,输出「分组 / 条数 / 大小」。
fn group_table(
    entries: &[AudioEntry],
    color: bool,
    key_header: &str,
    key: impl Fn(&str) -> String,
) -> String {
    let mut table = base_table(color);
    table.set_header(vec![
        head_cell(key_header, color),
        head_cell("条数", color),
        head_cell("大小", color),
    ]);
    for (name, count, bytes) in group_by(entries, key) {
        table.add_row(vec![
            Cell::new(name),
            Cell::new(count),
            size_cell(bytes, color),
        ]);
    }
    table.to_string()
}

/// 最旧 / 最新表(按 mtime)。无任何条目有 mtime 时返回 `None`。
fn extremes_table(entries: &[AudioEntry], now: SystemTime, color: bool) -> Option<String> {
    let dated = entries
        .iter()
        .filter_map(|e| e.mtime.map(|t| (e, t)))
        .collect::<Vec<_>>();
    let oldest = dated.iter().min_by_key(|(_, t)| *t)?;
    let newest = dated.iter().max_by_key(|(_, t)| *t)?;

    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("", color),
        head_cell("条目", color),
        head_cell("时间", color),
    ]);
    table.add_row(vec![
        label_cell("最旧", color),
        Cell::new(oldest.0.relpath.clone()),
        Cell::new(relative_age(now, oldest.1)),
    ]);
    table.add_row(vec![
        label_cell("最新", color),
        Cell::new(newest.0.relpath.clone()),
        Cell::new(relative_age(now, newest.1)),
    ]);
    Some(table.to_string())
}

/// 逐条清单(标题 / 音质 / 格式 / 大小 / 入库时间),按大小降序。
fn detail_table(entries: &[AudioEntry], now: SystemTime, color: bool) -> String {
    let mut rows = entries.iter().collect::<Vec<_>>();
    rows.sort_by_key(|e| std::cmp::Reverse(e.bytes));

    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("标题", color),
        head_cell("音质", color),
        head_cell("格式", color),
        head_cell("大小", color),
        head_cell("入库", color),
    ]);
    for e in rows {
        let age = e
            .mtime
            .map_or_else(|| "—".to_owned(), |t| relative_age(now, t));
        table.add_row(vec![
            Cell::new(audio_title(&e.relpath)),
            Cell::new(audio_quality(&e.relpath)),
            Cell::new(audio_ext(&e.relpath)),
            size_cell(e.bytes, color),
            Cell::new(age),
        ]);
    }
    table.to_string()
}

/// 给一段表格文本加 `▸ 标题` 前缀行。
fn labeled(title: &str, body: &str) -> String {
    format!("▸ {title}\n{body}")
}

/// 建带圆角边框、关闭动态排版(列宽随内容、不换行,保证确定性)的基底表。
fn base_table(color: bool) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Disabled);
    if color {
        table.enforce_styling();
    } else {
        table.force_no_tty();
    }
    table
}

/// 表头单元格:加粗。
fn head_cell(text: &str, color: bool) -> Cell {
    let cell = Cell::new(text);
    if color {
        cell.add_attribute(Attribute::Bold)
    } else {
        cell
    }
}

/// 行首标签单元格:加粗 + 青色。
fn label_cell(text: &str, color: bool) -> Cell {
    let cell = Cell::new(text);
    if color {
        cell.add_attribute(Attribute::Bold).fg(Color::Cyan)
    } else {
        cell
    }
}

/// 大小单元格:绿色。
fn size_cell(bytes: u64, color: bool) -> Cell {
    maybe_fg(Cell::new(human_bytes(bytes)), color, Color::Green)
}

/// 「已清空」单元格:暗灰。
fn cleared_cell(color: bool) -> Cell {
    maybe_fg(Cell::new("已清空"), color, Color::DarkGrey)
}

/// 条件上色:`color` 为真才给单元格上前景色。
fn maybe_fg(cell: Cell, color: bool, fg: Color) -> Cell {
    if color { cell.fg(fg) } else { cell }
}

/// 占用率分级配色:< 70% 绿、< 90% 黄、否则红。
fn level_color(pct: u64) -> Color {
    if pct < 70 {
        Color::Green
    } else if pct < 90 {
        Color::Yellow
    } else {
        Color::Red
    }
}

/// 计算占用率与进度条;容量缺失 / 为 0 返回 `None`。
///
/// # Return:
///   `Some((百分比, "[███░░░]" 条))`。
fn occupancy(used: u64, capacity: Option<u64>) -> Option<(u64, String)> {
    let cap = capacity.filter(|c| *c > 0)?;
    let pct = used
        .saturating_mul(100)
        .checked_div(cap)
        .unwrap_or(0)
        .min(100);
    let filled = (pct.saturating_mul(BAR_WIDTH) / 100).min(BAR_WIDTH);
    let filled_n = usize::try_from(filled).unwrap_or(0);
    let empty_n = usize::try_from(BAR_WIDTH.saturating_sub(filled)).unwrap_or(0);
    let bar = format!("[{}{}]", "█".repeat(filled_n), "░".repeat(empty_n));
    Some((pct, bar))
}

/// 字节数 → 人读字符串(B / KiB / MiB / GiB,一位小数)。
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        let tenths = bytes.saturating_mul(10) / GIB;
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    } else if bytes >= MIB {
        let tenths = bytes.saturating_mul(10) / MIB;
        format!("{}.{} MiB", tenths / 10, tenths % 10)
    } else if bytes >= KIB {
        let tenths = bytes.saturating_mul(10) / KIB;
        format!("{}.{} KiB", tenths / 10, tenths % 10)
    } else {
        format!("{bytes} B")
    }
}

/// 相对时间:`刚刚` / `N 分钟前` / `N 小时前` / `N 天前`。时光倒流(now < t)记为 `刚刚`。
fn relative_age(now: SystemTime, t: SystemTime) -> String {
    let secs = now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    if secs < 60 {
        "刚刚".to_owned()
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else if secs < 86400 {
        format!("{} 小时前", secs / 3600)
    } else {
        format!("{} 天前", secs / 86400)
    }
}

/// 取 relpath 末段文件名的扩展名;无扩展名记 `无扩展`。
fn audio_ext(relpath: &str) -> &str {
    let file = relpath.rsplit('/').next().unwrap_or(relpath);
    match file.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext,
        _ => "无扩展",
    }
}

/// 取 relpath 第 2 段(音质目录层);缺失记 `未知`。
fn audio_quality(relpath: &str) -> &str {
    relpath
        .split('/')
        .nth(1)
        .filter(|s| !s.is_empty())
        .unwrap_or("未知")
}

/// 取 relpath 末段文件名去扩展名后的主干(标题)。
fn audio_title(relpath: &str) -> &str {
    let file = relpath.rsplit('/').next().unwrap_or(relpath);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    }
}

/// 按 `key(relpath)` 聚合,返回按分组名排序的 `(分组, 条数, 总字节)`。
fn group_by(entries: &[AudioEntry], key: impl Fn(&str) -> String) -> Vec<(String, u64, u64)> {
    let mut map = BTreeMap::<String, (u64, u64)>::new();
    for e in entries {
        let slot = map.entry(key(&e.relpath)).or_insert((0, 0));
        slot.0 = slot.0.saturating_add(1);
        slot.1 = slot.1.saturating_add(e.bytes);
    }
    map.into_iter()
        .map(|(name, (count, bytes))| (name, count, bytes))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use mineral_persist::{CacheEntryStat, CacheStats, PlaylistCacheStats};

    use super::{
        AudioEntry, AudioInput, CoverInput, audio_ext, audio_quality, audio_title, human_bytes,
        occupancy, relative_age, render_clean, render_status,
    };

    /// 固定时间锚点:UNIX_EPOCH + 100 天,作渲染测试的 `now`。
    fn anchor() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(100 * 86400)
    }

    /// 造一条音频项,mtime = anchor 之前 `days_ago` 天。
    fn entry(relpath: &str, bytes: u64, days_ago: u64) -> AudioEntry {
        AudioEntry {
            relpath: relpath.to_owned(),
            bytes,
            mtime: Some(anchor() - Duration::from_secs(days_ago * 86400)),
        }
    }

    #[test]
    fn human_bytes_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MiB");
        assert_eq!(human_bytes(10 * 1024 * 1024 * 1024), "10.0 GiB");
    }

    #[test]
    fn occupancy_bar_and_pct() -> color_eyre::Result<()> {
        assert_eq!(occupancy(50, None), None, "无容量无占用率");
        assert_eq!(occupancy(50, Some(0)), None, "0 容量不除");
        let (pct, bar) =
            occupancy(30, Some(100)).ok_or_else(|| color_eyre::eyre::eyre!("有容量应给占用率"))?;
        assert_eq!(pct, 30);
        assert_eq!(bar, "[███░░░░░░░]");
        let (pct_full, _) =
            occupancy(200, Some(100)).ok_or_else(|| color_eyre::eyre::eyre!("超额"))?;
        assert_eq!(pct_full, 100, "占用率封顶 100");
        Ok(())
    }

    #[test]
    fn relpath_parsing() {
        let rel = "netease/lossless/叶惠美/晴天.flac";
        assert_eq!(audio_ext(rel), "flac");
        assert_eq!(audio_quality(rel), "lossless");
        assert_eq!(audio_title(rel), "晴天");
        assert_eq!(audio_ext("netease/exhigh/x/无后缀"), "无扩展");
    }

    #[test]
    fn relative_age_buckets() {
        let now = anchor();
        assert_eq!(relative_age(now, now), "刚刚");
        assert_eq!(
            relative_age(now, now - Duration::from_secs(120)),
            "2 分钟前"
        );
        assert_eq!(
            relative_age(now, now - Duration::from_secs(7200)),
            "2 小时前"
        );
        assert_eq!(
            relative_age(now, now - Duration::from_secs(3 * 86400)),
            "3 天前"
        );
    }

    #[test]
    fn status_summary_snapshot() {
        let audio = AudioInput {
            entries: vec![
                entry("netease/lossless/叶惠美/晴天.flac", 30 * 1024 * 1024, 5),
                entry("netease/exhigh/范特西/双截棍.mp3", 8 * 1024 * 1024, 1),
            ],
            total_bytes: 38 * 1024 * 1024,
            capacity: Some(10 * 1024 * 1024 * 1024),
        };
        let cover = CoverInput {
            count: 12,
            total_bytes: 4 * 1024 * 1024,
            capacity: Some(1024 * 1024 * 1024),
        };
        let playlist = PlaylistCacheStats {
            playlists: 3,
            tracks: 120,
        };
        let out = render_status(
            &audio,
            &cover,
            &playlist,
            /*detail*/ false,
            /*color*/ false,
            anchor(),
        );
        mineral_test::assert_snap!(
            "cache status 汇总:音频 + 封面 + 歌单 + 分格式 + 最旧最新",
            out
        );
    }

    #[test]
    fn status_detail_snapshot() {
        let audio = AudioInput {
            entries: vec![
                entry("netease/lossless/叶惠美/晴天.flac", 30 * 1024 * 1024, 5),
                entry(
                    "netease/lossless/十一月的萧邦/夜曲.flac",
                    28 * 1024 * 1024,
                    9,
                ),
                entry("netease/exhigh/范特西/双截棍.mp3", 8 * 1024 * 1024, 1),
            ],
            total_bytes: 66 * 1024 * 1024,
            capacity: Some(10 * 1024 * 1024 * 1024),
        };
        let cover = CoverInput {
            count: 0,
            total_bytes: 0,
            capacity: Some(1024 * 1024 * 1024),
        };
        let playlist = PlaylistCacheStats {
            playlists: 0,
            tracks: 0,
        };
        let out = render_status(
            &audio,
            &cover,
            &playlist,
            /*detail*/ true,
            /*color*/ false,
            anchor(),
        );
        mineral_test::assert_snap!("cache status --detail:逐条清单 + 按音质分布", out);
    }

    #[test]
    fn clean_snapshot() {
        let audio = CacheStats {
            root: None,
            entries: vec![
                CacheEntryStat {
                    relpath: "netease/lossless/叶惠美/晴天.flac".to_owned(),
                    bytes: 30 * 1024 * 1024,
                },
                CacheEntryStat {
                    relpath: "netease/exhigh/范特西/双截棍.mp3".to_owned(),
                    bytes: 8 * 1024 * 1024,
                },
            ],
            total_bytes: 38 * 1024 * 1024,
            capacity: Some(0),
        };
        let cover = CacheStats {
            root: None,
            entries: vec![CacheEntryStat {
                relpath: "netease/abc.jpg".to_owned(),
                bytes: 4 * 1024 * 1024,
            }],
            total_bytes: 4 * 1024 * 1024,
            capacity: Some(0),
        };
        let playlist = PlaylistCacheStats {
            playlists: 3,
            tracks: 120,
        };
        let out = render_clean(&audio, &cover, &playlist, /*color*/ false);
        mineral_test::assert_snap!("cache clean:三区域前后对比 + 音频分格式 + 总释放", out);
    }
}
