//! `meta/config.lua` 的拼装:手写片段(preamble / aliases)+ 各 schema 段
//! 宏生成的 `---@class` / `---@alias` 常量,按主题序合成完整 LuaCATS stub。

use crate::schema::{
    AmbientConfig, AnchorConfig, AnimationConfig, AudioConfig, BackendKind, BackfillSection,
    BarsConfig, BehaviorConfig, BilibiliSection, CacheConfig, ChannelSearchConfig, Config,
    CopyConfig, CopyContext, CopyTemplate, CoverCacheConfig, CoverConfig, CoverProtocolMode,
    CoverStorageMode, CoverTransitionConfig, CoverTransitionStyle, DaemonConfig, DeepSearchConfig,
    DeepWeights, DownloadConfig, DriftConfig, DynamicThemeConfig, EnvelopeConfig, HighpassConfig,
    KeysConfig, KittyTransmitConfig, KmeansConfig, LayoutConfig, LyricsConfig, MarqueeBounceConfig,
    MarqueeConfig, MarqueeLoopConfig, MarqueeMode, MenuReveal, MineralSection, NeteaseSection,
    PrefetchConfig, ReportConfig, RotateConfig, ScopeConfig, ScriptConfig, SearchConfig,
    SearchFocusTransition, SearchHitConfig, SearchQueryMode, ShelfConfig, SourcesConfig,
    SpectrumConfig, SpectrumStyle, StatsConfig, StatsLevel, SweepStyle, TerrainConfig, TextStyle,
    ThemeConfig, TitleField, TitleIcons, ToastConfig, TrackPosMemory, TuiConfig, VignetteConfig,
    WaterfallConfig, WaveformConfig, WindowTitleConfig, ZoomConfig,
};

/// 文件头:`---@meta` 声明 + 使用说明(手写 prose,不随 schema 变)。
const PREAMBLE: &str = include_str!("lua/meta/preamble.lua");

/// 手写别名片段:语法型值(联合形态)与 untagged 复合枚举,无法从单一
/// Rust 类型投影,与各自的自定义 Deserialize 同步维护。
const ALIASES: &str = include_str!("lua/meta/aliases.lua");

/// 拼装完整的 `meta/config.lua` 文本:preamble → 手写 aliases → 宏生成的
/// 枚举 alias → 宏生成的配置段 class(主题序)。新增 config_section /
/// lua_enum 类型必须挂进本清单——漏挂时若有字段引用它,闭合性测试红;
/// 删除类型则清单直接编译错。
///
/// # Return:
///   stub 全文(带尾随换行,可直接落盘)
pub(crate) fn meta_config_lua() -> String {
    let enum_aliases = [
        SpectrumStyle::LUA_ALIAS,
        BackendKind::LUA_ALIAS,
        TrackPosMemory::LUA_ALIAS,
        CoverProtocolMode::LUA_ALIAS,
        CoverStorageMode::LUA_ALIAS,
        MarqueeMode::LUA_ALIAS,
        SweepStyle::LUA_ALIAS,
        MenuReveal::LUA_ALIAS,
        SearchFocusTransition::LUA_ALIAS,
        CoverTransitionStyle::LUA_ALIAS,
        TextStyle::LUA_ALIAS,
        CopyContext::LUA_ALIAS,
        TitleField::LUA_ALIAS,
        StatsLevel::LUA_ALIAS,
        SearchQueryMode::LUA_ALIAS,
    ]
    .join("\n\n");
    let classes = [
        Config::LUA_STUB,
        TuiConfig::LUA_STUB,
        ThemeConfig::LUA_STUB,
        DynamicThemeConfig::LUA_STUB,
        SearchHitConfig::LUA_STUB,
        KeysConfig::LUA_STUB,
        BehaviorConfig::LUA_STUB,
        SpectrumConfig::LUA_STUB,
        BarsConfig::LUA_STUB,
        ScopeConfig::LUA_STUB,
        WaterfallConfig::LUA_STUB,
        TerrainConfig::LUA_STUB,
        WaveformConfig::LUA_STUB,
        CoverConfig::LUA_STUB,
        CoverCacheConfig::LUA_STUB,
        KittyTransmitConfig::LUA_STUB,
        KmeansConfig::LUA_STUB,
        CoverTransitionConfig::LUA_STUB,
        ZoomConfig::LUA_STUB,
        AmbientConfig::LUA_STUB,
        VignetteConfig::LUA_STUB,
        DriftConfig::LUA_STUB,
        RotateConfig::LUA_STUB,
        AnchorConfig::LUA_STUB,
        PrefetchConfig::LUA_STUB,
        SearchConfig::LUA_STUB,
        DeepSearchConfig::LUA_STUB,
        DeepWeights::LUA_STUB,
        ChannelSearchConfig::LUA_STUB,
        LyricsConfig::LUA_STUB,
        AnimationConfig::LUA_STUB,
        MarqueeConfig::LUA_STUB,
        MarqueeLoopConfig::LUA_STUB,
        MarqueeBounceConfig::LUA_STUB,
        ToastConfig::LUA_STUB,
        LayoutConfig::LUA_STUB,
        CopyConfig::LUA_STUB,
        CopyTemplate::LUA_STUB,
        WindowTitleConfig::LUA_STUB,
        TitleIcons::LUA_STUB,
        AudioConfig::LUA_STUB,
        EnvelopeConfig::LUA_STUB,
        ShelfConfig::LUA_STUB,
        HighpassConfig::LUA_STUB,
        CacheConfig::LUA_STUB,
        DownloadConfig::LUA_STUB,
        SourcesConfig::LUA_STUB,
        NeteaseSection::LUA_STUB,
        BilibiliSection::LUA_STUB,
        MineralSection::LUA_STUB,
        BackfillSection::LUA_STUB,
        DaemonConfig::LUA_STUB,
        ScriptConfig::LUA_STUB,
        StatsConfig::LUA_STUB,
        ReportConfig::LUA_STUB,
    ]
    .join("\n\n");
    format!("{PREAMBLE}\n{ALIASES}\n{enum_aliases}\n\n{classes}\n")
}

#[cfg(test)]
mod tests {
    use crate::schema::{
        AudioConfig, Config, CopyTemplate, NeteaseSection, SourcesConfig, SpectrumStyle, TitleIcons,
    };

    /// 封闭 serde 枚举经 lua_enum 宏产出 alias 常量,变体值与落型一致。
    #[test]
    fn enum_aliases_generated() {
        assert_eq!(
            SpectrumStyle::LUA_ALIAS.lines().last().unwrap_or_default(),
            r#"---@alias mineral.SpectrumStyle "bars"|"scope"|"waterfall"|"terrain""#
        );
    }

    /// 落型前被摘走的函数字段经 lua_extra_field 声明进 stub:
    /// template 必填、curate_playlists 可选。
    #[test]
    fn extra_function_fields_declared() {
        assert!(
            CopyTemplate::LUA_STUB.contains("---@field template fun("),
            "CopyTemplate 应声明必填 template 函数字段:\n{}",
            CopyTemplate::LUA_STUB
        );
        assert!(
            NeteaseSection::LUA_STUB
                .contains("---@field curate_playlists? mineral.CuratePlaylistsFn"),
            "per-source 策展函数应可选:\n{}",
            NeteaseSection::LUA_STUB
        );
        assert!(
            SourcesConfig::LUA_STUB
                .contains("---@field curate_playlists? mineral.CuratePlaylistsFn"),
            "跨源策展函数应可选:\n{}",
            SourcesConfig::LUA_STUB
        );
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

    /// TitleIcons 转正为 config_section 后应有 stub 常量。
    #[test]
    fn title_icons_is_config_section() {
        assert!(
            TitleIcons::LUA_STUB.contains("---@class mineral.TitleIcons"),
            "TitleIcons 应经 config_section 生成 stub:\n{}",
            TitleIcons::LUA_STUB
        );
    }

    /// 闭合性:产物里引用的每个 `mineral.<Type>` 都必须有定义——本产物的
    /// `@class`/`@alias`,或 host stub(meta/mineral.lua)里的数据类型
    /// (Song / Playlist / PlaylistBrief 等)。新 struct 忘挂拼装清单、
    /// 字段引用了没写 alias 的叶子类型,都在这里红。
    #[test]
    fn assembled_stub_is_reference_closed() -> color_eyre::Result<()> {
        let assembled = super::meta_config_lua();
        let defined = type_names(&assembled, &["---@class ", "---@alias "]);
        let host = include_str!("lua/meta/mineral.lua");
        let host_defined = type_names(host, &["---@class ", "---@alias "]);
        let referenced = reference_names(&assembled);
        let undefined = referenced
            .iter()
            .filter(|name| !defined.contains(*name) && !host_defined.contains(*name))
            .collect::<Vec<_>>();
        assert!(undefined.is_empty(), "产物引用了未定义的类型:{undefined:?}");
        Ok(())
    }

    /// 提取 `@class` / `@alias` 声明的类型名集合。
    fn type_names(text: &str, markers: &[&str]) -> rustc_hash::FxHashSet<String> {
        text.lines()
            .filter_map(|line| {
                markers.iter().find_map(|marker| {
                    let rest = line.trim_start().strip_prefix(marker)?;
                    let name = rest.split_whitespace().next()?;
                    // `---@class mineral.Foo: mineral.Bar` 继承形:名字在冒号前。
                    Some(name.split(':').next().unwrap_or(name).to_owned())
                })
            })
            .collect()
    }

    /// 提取产物中出现的全部 `mineral.<大写开头>` 类型引用(小写开头的
    /// `mineral.player` 等是 host API 命名空间,不是类型引用)。
    fn reference_names(text: &str) -> rustc_hash::FxHashSet<String> {
        let mut out = rustc_hash::FxHashSet::<String>::default();
        for (start, _) in text.match_indices("mineral.") {
            let rest = text.get(start + "mineral.".len()..).unwrap_or_default();
            let name = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>();
            if name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                out.insert(format!("mineral.{name}"));
            }
        }
        out
    }

    /// stub 是用户面文档:Rust 实现细节的样板句(可见性/getter 约定)与 rustdoc
    /// 链接语法不该漏进来。防回归:新 struct 文档再写这些,这里红。
    #[test]
    fn stub_free_of_rust_only_boilerplate() {
        let assembled = super::meta_config_lua();
        assert!(
            !assembled.contains("字段私有"),
            "可见性样板句不该进用户面 stub"
        );
        assert!(
            !assembled.contains("non_exhaustive"),
            "Rust 属性名不该进用户面 stub"
        );
        assert!(!assembled.contains("[`"), "rustdoc 链接语法应被宏剥除");
    }

    /// 跨 crate 枚举(mineral-model)挂不了 lua_enum 宏,alias 手写在 aliases.lua;
    /// 逐变体 serde 序列化与 alias 字面量比对,钉住值集与顺序,防两边漂移。
    #[test]
    fn model_enum_aliases_match_variants() -> color_eyre::Result<()> {
        assert_eq!(
            alias_literals("mineral.BitRate")?,
            serialized_variants(&mineral_model::BitRate::ALL)?,
            "BitRate alias 字面量应与变体序列化一致(升序)"
        );
        assert_eq!(
            alias_literals("mineral.SearchKind")?,
            serialized_variants(&mineral_model::SearchKind::ALL)?,
            "SearchKind alias 字面量应与变体序列化一致(声明序)"
        );
        Ok(())
    }

    /// 从手写 aliases 片段提取某 alias 的全部带引号字面量(按声明序)。
    fn alias_literals(name: &str) -> color_eyre::Result<Vec<String>> {
        use color_eyre::eyre::eyre;
        alias_members(name)?
            .into_iter()
            .map(|token| {
                token
                    .strip_prefix('"')
                    .and_then(|t| t.strip_suffix('"'))
                    .map(str::to_owned)
                    .ok_or_else(|| eyre!("{name} 含非字符串字面量 token:{token}"))
            })
            .collect()
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

    /// 手写 alias 的字面量成员必须被对应 Rust 类型的真实 Deserialize 接受
    /// (正向守卫:alias 写错字、或 Rust Visitor 收紧接受集,这里红)。
    /// 形态成员(数字 / 布尔)用代表值补充覆盖。
    #[test]
    fn handwritten_alias_literals_deserialize() -> color_eyre::Result<()> {
        use crate::schema::{AnsiSlot, MenuAlign, RetentionDays};
        // AnsiSlot:16 个槽名 + 数字槽号(0-15 合法,16 越界拒绝)。
        for slot_name in alias_literals("mineral.AnsiSlot").into_iter().flatten() {
            serde_json::from_value::<AnsiSlot>(serde_json::json!(slot_name))?;
        }
        serde_json::from_value::<AnsiSlot>(serde_json::json!(15))?;
        assert!(
            serde_json::from_value::<AnsiSlot>(serde_json::json!(16)).is_err(),
            "槽号 16 越界应拒"
        );
        // MenuAlign:三个关键字 + 数字比例。
        for keyword in alias_literals("mineral.MenuAlign").into_iter().flatten() {
            serde_json::from_value::<MenuAlign>(serde_json::json!(keyword))?;
        }
        serde_json::from_value::<MenuAlign>(serde_json::json!(0.5))?;
        // RetentionDays:false = 永久,正整数 = 天数;true 无意义应拒。
        let members = alias_members("mineral.RetentionDays")?;
        assert_eq!(
            members,
            ["false", "integer"],
            "RetentionDays 的形态成员应与 Visitor 接受集对应"
        );
        serde_json::from_value::<RetentionDays>(serde_json::json!(false))?;
        serde_json::from_value::<RetentionDays>(serde_json::json!(30))?;
        assert!(
            serde_json::from_value::<RetentionDays>(serde_json::json!(true)).is_err(),
            "retention true 应拒"
        );
        Ok(())
    }

    /// 逐变体 serde 序列化成字符串(与落型接受的值一致)。
    fn serialized_variants<T: serde::Serialize>(variants: &[T]) -> color_eyre::Result<Vec<String>> {
        use color_eyre::eyre::eyre;
        variants
            .iter()
            .map(|v| {
                let value = serde_json::to_value(v)?;
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| eyre!("变体应序列化为字符串,实得 {value}"))
            })
            .collect()
    }

    /// 产物快照:stub 全文是用户可见面,review 每次 schema 变更对它的影响。
    #[test]
    fn assembled_stub_snapshot() {
        mineral_test::assert_snap!(
            "meta/config.lua 生成全文(preamble + aliases + 宏生成 class/alias)",
            super::meta_config_lua()
        );
    }

    /// config_section 宏应为每个段生成 LUA_STUB 关联常量,含 class 头与字段行。
    #[test]
    fn config_section_emits_lua_stub_const() {
        assert!(
            AudioConfig::LUA_STUB.contains("---@class mineral.AudioConfig"),
            "缺 class 头:\n{}",
            AudioConfig::LUA_STUB
        );
        assert!(
            AudioConfig::LUA_STUB.contains("---@field volume? integer"),
            "缺字段行:\n{}",
            AudioConfig::LUA_STUB
        );
        assert!(
            Config::LUA_STUB.contains("---@field stats? mineral.StatsConfig"),
            "根 Config 应含 stats 段(现手写 stub 缺失的正是它):\n{}",
            Config::LUA_STUB
        );
    }

    /// source_section 注入的共用网络字段应进 stub,proxy 的 string|false
    /// 形态由注入点的 lua_type 覆盖表达。
    #[test]
    fn source_section_emits_injected_fields() {
        assert!(
            NeteaseSection::LUA_STUB.contains("---@field timeout_secs? integer"),
            "缺注入字段:\n{}",
            NeteaseSection::LUA_STUB
        );
        assert!(
            NeteaseSection::LUA_STUB.contains("---@field proxy? string|false"),
            "proxy 应为 string|false 覆盖形态:\n{}",
            NeteaseSection::LUA_STUB
        );
    }
}
