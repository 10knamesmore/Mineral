//! 从 TUI schema 的宏生成片段拼装本宿主 LuaLS 配置类型。

use super::schema::{
    AmbientConfig, AmbientTrailConfig, AnchorConfig, AnimationConfig, BarsConfig, BehaviorConfig,
    ChannelSearchConfig, CopyConfig, CopyContext, CopyTemplate, CoverCacheConfig, CoverCellFit,
    CoverConfig, CoverDecodePixelsConfig, CoverProtocolMode, CoverTransitionConfig,
    CoverTransitionStyle, DeepSearchConfig, DeepWeights, DriftConfig, DynamicThemeConfig,
    FilterPlayScope, FsSpectrumConfig, KeysConfig, KmeansConfig, LayoutConfig,
    LyricTextAlphaConfig, LyricsConfig, MarqueeBounceConfig, MarqueeConfig, MarqueeLoopConfig,
    MarqueeMode, MenuReveal, MinimapConfig, PlayheadConfig, PrefetchConfig, ProgressConfig,
    ProgressTrackConfig, PulseConfig, PulseDepthConfig, PunchConfig, RevealConfig, RotateConfig,
    ScopeConfig, SearchConfig, SearchFocusTransition, SearchHitConfig, SpectrumConfig,
    SpectrumStyle, SweepStyle, TerrainConfig, TextAlphaConfig, TextStyle, ThemeConfig, TitleField,
    TitleIcons, ToastConfig, TrackPosMemory, TrailTimingConfig, TransportFeedbackConfig, TuiConfig,
    TuiScriptConfig, VignetteConfig, WaterfallConfig, WaveformConfig, WindowTitleConfig,
    ZoomConfig,
};

/// LuaLS 文件头与使用说明。
const PREAMBLE: &str = include_str!("lua/meta/preamble.lua");

/// 无法从单一 Rust 类型投影的联合形态,与自定义 Deserialize 同步。
const ALIASES: &str = include_str!("lua/meta/aliases.lua");

/// 拼装本宿主类型;新增宏生成类型须加入清单,引用闭合由 CLI 资产测试验证。
pub(super) fn meta_config_lua() -> String {
    let enum_aliases = [
        SpectrumStyle::LUA_ALIAS,
        TrackPosMemory::LUA_ALIAS,
        FilterPlayScope::LUA_ALIAS,
        CoverProtocolMode::LUA_ALIAS,
        CoverCellFit::LUA_ALIAS,
        MarqueeMode::LUA_ALIAS,
        SweepStyle::LUA_ALIAS,
        MenuReveal::LUA_ALIAS,
        SearchFocusTransition::LUA_ALIAS,
        CoverTransitionStyle::LUA_ALIAS,
        TextStyle::LUA_ALIAS,
        CopyContext::LUA_ALIAS,
        TitleField::LUA_ALIAS,
    ]
    .join("\n\n");
    let classes = [
        TuiConfig::LUA_STUB,
        TuiScriptConfig::LUA_STUB,
        ThemeConfig::LUA_STUB,
        DynamicThemeConfig::LUA_STUB,
        SearchHitConfig::LUA_STUB,
        TextAlphaConfig::LUA_STUB,
        KeysConfig::LUA_STUB,
        BehaviorConfig::LUA_STUB,
        SpectrumConfig::LUA_STUB,
        BarsConfig::LUA_STUB,
        ScopeConfig::LUA_STUB,
        WaterfallConfig::LUA_STUB,
        TerrainConfig::LUA_STUB,
        ProgressConfig::LUA_STUB,
        ProgressTrackConfig::LUA_STUB,
        PlayheadConfig::LUA_STUB,
        WaveformConfig::LUA_STUB,
        RevealConfig::LUA_STUB,
        CoverConfig::LUA_STUB,
        CoverDecodePixelsConfig::LUA_STUB,
        CoverCacheConfig::LUA_STUB,
        KmeansConfig::LUA_STUB,
        CoverTransitionConfig::LUA_STUB,
        ZoomConfig::LUA_STUB,
        AmbientConfig::LUA_STUB,
        VignetteConfig::LUA_STUB,
        DriftConfig::LUA_STUB,
        RotateConfig::LUA_STUB,
        PulseConfig::LUA_STUB,
        PulseDepthConfig::LUA_STUB,
        PunchConfig::LUA_STUB,
        AnchorConfig::LUA_STUB,
        PrefetchConfig::LUA_STUB,
        SearchConfig::LUA_STUB,
        DeepSearchConfig::LUA_STUB,
        DeepWeights::LUA_STUB,
        ChannelSearchConfig::LUA_STUB,
        LyricsConfig::LUA_STUB,
        LyricTextAlphaConfig::LUA_STUB,
        AnimationConfig::LUA_STUB,
        TransportFeedbackConfig::LUA_STUB,
        AmbientTrailConfig::LUA_STUB,
        TrailTimingConfig::LUA_STUB,
        MarqueeConfig::LUA_STUB,
        MarqueeLoopConfig::LUA_STUB,
        MarqueeBounceConfig::LUA_STUB,
        MinimapConfig::LUA_STUB,
        ToastConfig::LUA_STUB,
        LayoutConfig::LUA_STUB,
        FsSpectrumConfig::LUA_STUB,
        CopyConfig::LUA_STUB,
        CopyTemplate::LUA_STUB,
        WindowTitleConfig::LUA_STUB,
        TitleIcons::LUA_STUB,
    ]
    .join("\n\n");
    format!("{PREAMBLE}\n{ALIASES}\n{enum_aliases}\n\n{classes}\n")
}

#[cfg(test)]
mod tests {
    use crate::config::schema::{CopyTemplate, TuiConfig};

    /// 本地根类型、setup 参数与必填复制回调的机器类型定义保持一致。
    #[test]
    fn root_and_callbacks_are_declared() {
        assert!(TuiConfig::LUA_STUB.contains("---@class mineral.TuiConfig"));
        assert!(TuiConfig::LUA_STUB.contains("---@field theme? mineral.ThemeConfig"));
        assert!(!TuiConfig::LUA_STUB.contains("---@field tui?"));
        assert!(TuiConfig::LUA_STUB.contains("---@field setup? fun(api: mineral.TuiApi)"));
        assert!(CopyTemplate::LUA_STUB.contains("---@field template fun("));
    }

    /// CopyTemplate 是数组元素(不过深合并):可选性按 serde 真实语义,
    /// label 必填、key/context 可选。
    #[test]
    fn copy_template_optionality_by_serde() {
        assert!(
            CopyTemplate::LUA_STUB.contains("---@field label string"),
            "label 应必填(无 ?):\n{}",
            CopyTemplate::LUA_STUB
        );
        assert!(
            CopyTemplate::LUA_STUB.contains("---@field key? string"),
            "key 应可选:\n{}",
            CopyTemplate::LUA_STUB
        );
    }

    /// 手写 alias 的字面量成员必须被对应 Rust 类型的真实 Deserialize 接受
    /// (正向守卫:alias 写错字、或 Rust Visitor 收紧接受集,这里红)。
    /// 形态成员(数字 / 布尔)用代表值补充覆盖。
    #[test]
    fn handwritten_alias_literals_deserialize() -> color_eyre::Result<()> {
        use crate::config::schema::{AnsiSlot, MenuAlign};
        // AnsiSlot:16 个槽名 + 数字槽号(0-15 合法,16 越界拒绝)。
        let slots = alias_members("mineral.AnsiSlot")?;
        assert!(slots.len() > 1, "AnsiSlot 应同时声明槽名与数字槽号");
        assert_eq!(slots.last().map(String::as_str), Some("integer"));
        for token in slots.iter().filter(|token| token.as_str() != "integer") {
            serde_json::from_str::<AnsiSlot>(token)?;
        }
        serde_json::from_value::<AnsiSlot>(serde_json::json!(15))?;
        assert!(
            serde_json::from_value::<AnsiSlot>(serde_json::json!(16)).is_err(),
            "槽号 16 越界应拒"
        );
        // MenuAlign:三个关键字 + 数字比例。
        let alignments = alias_members("mineral.MenuAlign")?;
        assert!(alignments.len() > 1, "MenuAlign 应同时声明关键字与数字比例");
        assert_eq!(alignments.last().map(String::as_str), Some("number"));
        for token in alignments.iter().filter(|token| token.as_str() != "number") {
            serde_json::from_str::<MenuAlign>(token)?;
        }
        serde_json::from_value::<MenuAlign>(serde_json::json!(0.5))?;
        Ok(())
    }

    /// 从手写 aliases 片段提取某 alias 的全部 union 成员 token(按声明序,原样)。
    fn alias_members(name: &str) -> color_eyre::Result<Vec<String>> {
        use color_eyre::eyre::eyre;
        let marker = format!("---@alias {name} ");
        let line = super::ALIASES
            .lines()
            .find_map(|line| line.strip_prefix(&marker))
            .ok_or_else(|| eyre!("aliases.lua 缺 {name} 定义"))?;
        Ok(line
            .split('|')
            .map(|token| token.trim().to_owned())
            .collect())
    }
}
