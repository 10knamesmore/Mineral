//! 从 daemon schema 的宏生成片段拼装本宿主 LuaLS 配置类型。

use super::schema::{
    AudioConfig, BackendKind, BackfillSection, BilibiliSection, CacheConfig, DaemonConfig,
    DownloadConfig, EnvelopeConfig, HighpassConfig, LocalSection, MineralSection, NeteaseSection,
    PlaylistFetchSection, QueueConfig, QueueTransform, ReportConfig, ScriptConfig, SearchQueryMode,
    ShelfConfig, SourcesConfig, StatsConfig, StatsLevel,
};

/// LuaLS 文件头与使用说明。
const PREAMBLE: &str = include_str!("lua/meta/preamble.lua");

/// 无法从单一 Rust 类型投影的联合形态,与自定义 Deserialize 同步。
const ALIASES: &str = include_str!("lua/meta/aliases.lua");

/// 拼装本宿主类型;新增宏生成类型须加入清单,引用闭合由 CLI 资产测试验证。
pub(super) fn meta_config_lua() -> String {
    let enum_aliases = [
        BackendKind::LUA_ALIAS,
        StatsLevel::LUA_ALIAS,
        SearchQueryMode::LUA_ALIAS,
    ]
    .join("\n\n");
    let classes = [
        DaemonConfig::LUA_STUB,
        AudioConfig::LUA_STUB,
        EnvelopeConfig::LUA_STUB,
        ShelfConfig::LUA_STUB,
        HighpassConfig::LUA_STUB,
        CacheConfig::LUA_STUB,
        DownloadConfig::LUA_STUB,
        SourcesConfig::LUA_STUB,
        LocalSection::LUA_STUB,
        QueueConfig::LUA_STUB,
        QueueTransform::LUA_STUB,
        NeteaseSection::LUA_STUB,
        PlaylistFetchSection::LUA_STUB,
        BilibiliSection::LUA_STUB,
        MineralSection::LUA_STUB,
        BackfillSection::LUA_STUB,
        ScriptConfig::LUA_STUB,
        StatsConfig::LUA_STUB,
        ReportConfig::LUA_STUB,
    ]
    .join("\n\n");
    format!("{PREAMBLE}\n{ALIASES}\n{enum_aliases}\n\n{classes}\n")
}

#[cfg(test)]
mod tests {
    use crate::config::schema::{DaemonConfig, NeteaseSection, SourcesConfig};

    /// 配置根和 setup 只声明 daemon 类型,函数字段在落型前摘走。
    #[test]
    fn root_and_callbacks_are_declared() {
        assert!(DaemonConfig::LUA_STUB.contains("---@class mineral.DaemonConfig"));
        assert!(!DaemonConfig::LUA_STUB.contains("---@field daemon?"));
        assert!(!DaemonConfig::LUA_STUB.contains("---@field tui?"));
        assert!(DaemonConfig::LUA_STUB.contains("---@field gapless_prefetch_ms? integer"));
        assert!(DaemonConfig::LUA_STUB.contains("---@field setup? fun(api: mineral.DaemonApi)"));
        assert!(
            NeteaseSection::LUA_STUB
                .contains("---@field curate_playlists? mineral.CuratePlaylistsFn")
        );
        assert!(
            SourcesConfig::LUA_STUB
                .contains("---@field curate_playlists? mineral.CuratePlaylistsFn")
        );
    }

    /// false 表示永久保留,正整数表示天数;LuaLS 联合形态与落型一致。
    #[test]
    fn retention_alias_deserializes() -> color_eyre::Result<()> {
        use crate::config::schema::RetentionDays;
        assert_eq!(
            alias_members("mineral.RetentionDays")?,
            ["false", "integer"]
        );
        serde_json::from_value::<RetentionDays>(serde_json::json!(false))?;
        serde_json::from_value::<RetentionDays>(serde_json::json!(30))?;
        assert!(serde_json::from_value::<RetentionDays>(serde_json::json!(true)).is_err());
        Ok(())
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
