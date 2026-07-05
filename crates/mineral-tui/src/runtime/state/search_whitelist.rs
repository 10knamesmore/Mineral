//! channel 搜索下拉的配置白名单:类型 + 过滤/定序规则。
//!
//! 白名单语义:列出即暴露、顺序即下拉顺序。默认名单的唯一真相在 default.lua(代码不再
//! 持有一份);空列表走逐层防呆回退——配置滤空时宁可全暴露,不让搜索页变空壳。

use mineral_channel_core::ChannelCaps;
use mineral_model::{SearchKind, SourceKind};
use rustc_hash::FxHashMap;

/// channel 搜索两个下拉的白名单(`tui.search.sources` / `kinds` 的运行时快照)。
/// 配置每次运行不可变,构造 SearchPage 时拷一份进来,下拉数据源每帧据此重算。
#[derive(Clone, Debug, Default)]
pub(crate) struct SearchWhitelist {
    /// source 名单(按 [`SourceKind::name`] 匹配;没加载的名字静默跳过,同一份配置跨机器可移植)。
    pub(crate) sources: Vec<String>,

    /// kind 名单(与各 source 声明的 searchable 求交,保此处顺序)。
    pub(crate) kinds: Vec<SearchKind>,
}

impl From<&mineral_config::SearchConfig> for SearchWhitelist {
    fn from(cfg: &mineral_config::SearchConfig) -> Self {
        Self {
            sources: cfg.sources().clone(),
            kinds: cfg.kinds().clone(),
        }
    }
}

/// `caps` 过 kind 白名单后的可搜 kind 交集(保白名单顺序);空名单 = searchable 原序全量
/// (测试裸构造 / 用户显式写空列表都落这条防呆)。
///
/// # Params:
///   - `whitelist`: 下拉白名单
///   - `caps`: 单个 source 的能力声明
///
/// # Return:
///   过滤后的 kind 列表(可能为空——调用方据此隐藏该 source 或走回退)。
pub(crate) fn whitelisted_kinds(
    whitelist: &SearchWhitelist,
    caps: &ChannelCaps,
) -> Vec<SearchKind> {
    if whitelist.kinds.is_empty() {
        return caps.searchable().clone();
    }
    whitelist
        .kinds
        .iter()
        .copied()
        .filter(|kind| caps.searchable().contains(kind))
        .collect()
}

/// 可搜索的 source 列表(返回序即下拉展示序)。
///
/// 1. 基底 = `searchable` 非空的 source;再滤掉与 kind 白名单交集为空的(用户显式只搜
///    某几类,搜不了这些类的 source 整体消失)。kind 配置滤光一切 → 忽略 kind 配置。
/// 2. source 白名单定序过滤(按 `name()` 匹配,没加载的名字静默跳过,重复只收一次);
///    名单为空或滤空 → 回退第 1 步结果按 `name()` 字典序(去抖 FxHashMap 迭代序不确定)。
///
/// # Params:
///   - `whitelist`: 下拉白名单
///   - `caps`: 各 source 能力声明
///
/// # Return:
///   定序后的可搜 source 列表。
pub(crate) fn source_options(
    whitelist: &SearchWhitelist,
    caps: &FxHashMap<SourceKind, ChannelCaps>,
) -> Vec<SourceKind> {
    let base: Vec<SourceKind> = caps
        .iter()
        .filter(|(_, channel_caps)| !channel_caps.searchable().is_empty())
        .map(|(source, _)| *source)
        .collect();
    let mut pool: Vec<SourceKind> = base
        .iter()
        .copied()
        .filter(|source| {
            caps.get(source)
                .is_some_and(|channel_caps| !whitelisted_kinds(whitelist, channel_caps).is_empty())
        })
        .collect();
    if pool.is_empty() {
        pool = base;
    }
    let mut ordered = Vec::<SourceKind>::new();
    for name in &whitelist.sources {
        // 名单里重复写同一 source 只收一次,下拉不出重项。
        if let Some(source) = pool.iter().copied().find(|source| source.name() == name)
            && !ordered.contains(&source)
        {
            ordered.push(source);
        }
    }
    if !ordered.is_empty() {
        return ordered;
    }
    pool.sort_by_key(SourceKind::name);
    pool
}

/// `source` 的默认 kind:白名单过滤后的首项;交集空(回退态)退 searchable 首项。
///
/// # Params:
///   - `whitelist`: 下拉白名单
///   - `caps`: 各 source 能力声明
///   - `source`: 目标 source
///
/// # Return:
///   进入该 source 新会话时的初始 kind。
pub(crate) fn default_kind(
    whitelist: &SearchWhitelist,
    caps: &FxHashMap<SourceKind, ChannelCaps>,
    source: SourceKind,
) -> SearchKind {
    let Some(channel_caps) = caps.get(&source) else {
        return SearchKind::Song;
    };
    whitelisted_kinds(whitelist, channel_caps)
        .first()
        .copied()
        .or_else(|| channel_caps.searchable().first().copied())
        .unwrap_or(SearchKind::Song)
}
