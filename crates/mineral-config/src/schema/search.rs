//! 搜索段(挂在 `TuiConfig` 下):本地过滤搜索的行为旋钮 + channel 搜索的 source/kind 白名单。

use mineral_config_macros::config_section;
use mineral_model::SearchKind;

/// 搜索配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct SearchConfig {
    /// Playlists 视图搜索是否穿透到歌单内歌曲(总开关)。
    deep: bool,

    /// 深度搜索各字段的命中分折扣(字段级独立配置)。
    deep_weights: DeepWeights,

    /// 深度命中行 Enter 进歌单后,光标是否直接定位到命中歌(`false` = 仍从头看)。
    locate_on_enter: bool,

    /// channel 搜索的 source 白名单:列出即暴露、顺序即下拉顺序,未列出的隐藏。
    /// source 名是开放 string(插件源可写),没加载的名字运行时静默跳过——同一份配置
    /// 跨机器可移植。空列表 = 消费侧防呆回退全量(按名字典序)。
    sources: Vec<String>,

    /// channel 搜索的 kind 白名单:与各 source 声明的可搜集合求交,保配置顺序。
    /// 封闭枚举,typo 在加载期报落型告警。空列表 = 消费侧防呆回退全量(各 source 声明序)。
    kinds: Vec<SearchKind>,
}

/// 深度搜索的字段级权重。每项 0~1(越界 clamp),`0` = 该字段不参与匹配。
///
/// 歌单最终分 = max(歌单名分, 歌单内最佳歌曲分),
/// 单曲分 = max(name 权重 × 歌名分, artist 权重 × 艺人分, album 权重 × 专辑分)。
#[config_section]
pub struct DeepWeights {
    /// 歌名命中分折扣。
    name: f32,

    /// 艺人名命中分折扣(多艺人取最高)。
    artist: f32,

    /// 专辑名命中分折扣。
    album: f32,
}
