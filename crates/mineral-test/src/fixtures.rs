//! 展示性歌曲 fixtures:用真实曲目做测试数据,快照里一眼能认。

use mineral_model::Song;

use crate::builders::{song, with_album, with_alias, with_artist, with_duration};

/// Mineral 乐队专辑《EndSerenading》(1998)前 `n` 首(带艺人 + 真实时长)。
///
/// 用作展示性测试数据 —— 本播放器项目名正取自这支 emo 乐队。`n` 超过 10 时只给 10 首。
///
/// # Params:
///   - `n`: 取前几首
///
/// # Return:
///   前 `n` 首 `Song`(来源 Netease,艺人 "Mineral")。
pub fn endserenading(n: usize) -> Vec<Song> {
    const TRACKS: [(&str, &str, u64); 10] = [
        ("1", "LoveLetterTypewriter", 225_000),
        ("2", "Palisade", 271_000),
        ("3", "Gjs", 286_000),
        ("4", "Unfinished", 367_000),
        ("5", "ForIvadell", 216_000),
        ("6", "WakingToWinter", 242_000),
        ("7", "ALetter", 293_000),
        ("8", "SoundsLikeSunday", 320_000),
        ("9", "&serenading", 324_000),
        ("10", "TheLastWordIsRejoice", 309_000),
    ];
    TRACKS
        .iter()
        .take(n)
        .map(|&(id, name, dur)| with_artist(with_duration(named(id, name), dur), "Mineral"))
        .collect()
}

/// Chinese Football 乐队同名专辑(2015)前 `n` 首,用作 **CJK 宽字符**测试数据
/// —— 含「不是人人都能穿十号球衣」「地球上最后一个EMO男孩」这类长 CJK / 中英混排。
///
/// # Params:
///   - `n`: 取前几首
///
/// # Return:
///   前 `n` 首 `Song`(来源 Netease,艺人 "Chinese Football",统一时长 233s)。
pub fn chinese_football(n: usize) -> Vec<Song> {
    const TRACKS: [(&str, &str); 10] = [
        ("c1", "守门员"),
        ("c2", "飞鱼转身"),
        ("c3", "400米"),
        ("c4", "不是人人都能穿十号球衣"),
        ("c5", "世界悲"),
        ("c6", "地球上最后一个EMO男孩"),
        ("c7", "红牌罚下"),
        ("c8", "帽子戏法"),
        ("c9", "再见米卢"),
        ("c10", "盲人摸象"),
    ];
    TRACKS
        .iter()
        .take(n)
        .map(|&(id, name)| with_artist(with_duration(named(id, name), 233_000), "Chinese Football"))
        .collect()
}

/// 一首带**真实网易云别名**的歌:歌名「迷星叫」/ 别名「Mayoiuta」(罗马音读法),
/// 艺人 MyGO!!!!!、专辑「迷跡波」,全部取自真实抓取样本(歌单 17880415607,非杜撰)。
/// 用于别名后缀渲染 / 别名搜索测试。
///
/// # Return:
///   `name = "迷星叫"`、`alias = Some("Mayoiuta")` 的 `Song`(来源 Netease)。
pub fn aliased_song() -> Song {
    with_alias(
        with_album(
            with_artist(with_duration(named("mixj", "迷星叫"), 211_373), "MyGO!!!!!"),
            "迷跡波",
        ),
        "Mayoiuta",
    )
}

/// 造一首指定 id + 歌名的 `Song`(其余默认)。fixtures 内部用。
///
/// # Params:
///   - `id`: 歌曲 ID
///   - `name`: 歌名
///
/// # Return:
///   `name` 被设好的最小 `Song`。
fn named(id: &str, name: &str) -> Song {
    let mut s = song(id);
    s.name = name.to_owned();
    s
}
