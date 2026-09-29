//! 搜索段(挂在 `TuiConfig` 下):`deep` 是本地过滤搜索(`/`)的行为旋钮,`channel`
//! 是 channel 远程搜索页两个下拉的白名单——两套互不相关的功能共享一张父表,
//! 各自的字段与消费点见 [`DeepSearchConfig`] / [`ChannelSearchConfig`]。

use mineral_config_macros::config_section;
use mineral_model::SearchKind;

/// 搜索配置。
#[config_section]
pub struct SearchConfig {
    /// 本地过滤搜索
    deep: DeepSearchConfig,

    /// 远程搜索
    channel: ChannelSearchConfig,
}

/// 歌单内歌曲搜索
#[config_section]
pub struct DeepSearchConfig {
    /// 是否搜索歌单内歌曲
    enabled: bool,

    /// 搜索字段权重
    weights: DeepWeights,

    /// 进入歌单时定位命中曲
    locate_on_enter: bool,
}

/// 字段匹配权重 0-1；0 为不匹配
#[config_section]
pub struct DeepWeights {
    /// 歌名权重
    name: f32,

    /// 别名权重
    alias: f32,

    /// 艺人名权重
    artist: f32,

    /// 专辑名权重
    album: f32,
}

/// 远程搜索
#[config_section]
pub struct ChannelSearchConfig {
    /// 选中项详情加载防抖
    detail_debounce_ms: u64,

    /// 来源及排列顺序；未加载的跳过，空表显示全部
    #[lua_type("mineral.SourceName[]")]
    sources: Vec<String>,

    /// 搜索类型及顺序；仅显示来源支持项，空表显示全部
    kinds: Vec<SearchKind>,
}
