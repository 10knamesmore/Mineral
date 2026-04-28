//! 命令栏的解析与"效果"映射。
//!
//! 命令(`:` 前缀)被解析成一组 [`CmdEffect`],由 [`crate::app::App`] 应用。
//! 解析失败用 [`CmdEffect::Hint`] 回吐到底栏 hint 槽。

use crate::playback::{PlayMode, SortBy};

/// 命令栏当前模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdMode {
    /// `/` 实时过滤当前视图。
    Search,
    /// `:` 一次性命令。
    Command,
}

/// 单条命令的执行结果(纯数据,App 拿到后再 mutate state)。
#[derive(Clone, Debug)]
pub enum CmdEffect {
    /// 退出应用。
    Quit,
    /// 切播放模式。
    SetMode(PlayMode),
    /// 切排序模式。
    SetSort(SortBy),
    /// 切 accent 配色对(mauve/peach/green/...)。
    SetAccent(String),
    /// 切主题(stage 9 实装)。
    SetTheme(String),
    /// 播放 library 第 n 条(从 1 起)。
    Play(Option<usize>),
    /// 在底栏 hint 槽显示一条临时消息。
    Hint(String),
}

/// 解析一条 `:` 命令。返回的 effect 列表通常只有 1 条;参数错误回吐 hint。
pub fn parse(cmd: &str) -> Vec<CmdEffect> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut iter = trimmed.split_whitespace();
    let head = iter.next().unwrap_or("");
    let rest: Vec<&str> = iter.collect();
    match head {
        "q" | "quit" => vec![CmdEffect::Quit],
        "mode" => parse_mode(rest.first().copied()),
        "sort" => parse_sort(rest.first().copied()),
        "accent" => rest.first().map_or_else(
            || {
                vec![CmdEffect::Hint(
                    "usage: :accent {mauve|peach|green|...}".to_owned(),
                )]
            },
            |s| vec![CmdEffect::SetAccent((*s).to_owned())],
        ),
        "theme" => rest.first().map_or_else(
            || {
                vec![CmdEffect::Hint(
                    "usage: :theme {mocha|macchiato|frappe|latte}".to_owned(),
                )]
            },
            |s| vec![CmdEffect::SetTheme((*s).to_owned())],
        ),
        "play" => {
            let n = rest.first().and_then(|s| s.parse::<usize>().ok());
            vec![CmdEffect::Play(n)]
        }
        _ => vec![CmdEffect::Hint(format!("unknown command: {head}"))],
    }
}

fn parse_mode(arg: Option<&str>) -> Vec<CmdEffect> {
    let usage = "usage: :mode {seq|shuffle|repeat-all|repeat-one}".to_owned();
    match arg {
        Some("seq" | "sequential") => vec![CmdEffect::SetMode(PlayMode::Sequential)],
        Some("shuffle") => vec![CmdEffect::SetMode(PlayMode::Shuffle)],
        Some("repeat-all" | "repeatall") => vec![CmdEffect::SetMode(PlayMode::RepeatAll)],
        Some("repeat-one" | "repeatone") => vec![CmdEffect::SetMode(PlayMode::RepeatOne)],
        _ => vec![CmdEffect::Hint(usage)],
    }
}

fn parse_sort(arg: Option<&str>) -> Vec<CmdEffect> {
    let usage = "usage: :sort {default|title|artist|plays|length|year}".to_owned();
    match arg {
        Some("default") => vec![CmdEffect::SetSort(SortBy::Default)],
        Some("title") => vec![CmdEffect::SetSort(SortBy::Title)],
        Some("artist") => vec![CmdEffect::SetSort(SortBy::Artist)],
        Some("plays") => vec![CmdEffect::SetSort(SortBy::Plays)],
        Some("length" | "len") => vec![CmdEffect::SetSort(SortBy::Length)],
        Some("year") => vec![CmdEffect::SetSort(SortBy::Year)],
        _ => vec![CmdEffect::Hint(usage)],
    }
}
