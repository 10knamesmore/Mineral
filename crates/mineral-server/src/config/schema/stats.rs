//! stats 段:行为埋点的采集旋钮。
//!
//! 采集侧 vs 口径侧分层:采集旋钮(level / collect / search_queries / exclude_sources /
//! session_gap_minutes / retention_days)影响落库、只对未来生效;`report` 子表是查询期
//! 口径,改动可回溯重算全部历史。分界判据:改了这个旋钮,历史数据的含义变不变。

use mineral_config_macros::{config_section, lua_enum};
use rustc_hash::FxHashMap;
use serde::Deserialize;

/// core 本体表名:`collect` 逐键微调不得覆盖它们(开关只认 `level`)。
const CORE_TABLES: [&str; 2] = ["plays", "sessions"];

/// 反序列化 `collect` map,并**拒绝对 core 本体(plays / sessions)的覆盖**:关闭播放 / 会话
/// 的唯一途径是 `level = "off"`,不能经 collect 逐键关——否则「level=full + collect.plays=false」
/// 是自相矛盾的半关态。未知 kind 名不在此拒(合法 kind 集在埋点层,留给 daemon 应用时按集警告)。
fn deserialize_collect<'de, D>(deserializer: D) -> Result<FxHashMap<String, bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let map = FxHashMap::<String, bool>::deserialize(deserializer)?;
    for core in CORE_TABLES {
        if map.contains_key(core) {
            return Err(serde::de::Error::custom(format!(
                "collect 不能覆盖 core 本体 {core:?}(关闭播放 / 会话请用 level = \"off\")"
            )));
        }
    }
    Ok(map)
}

/// 统计采集范围
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatsLevel {
    /// 不采集
    Off,

    /// 播放与会话
    Core,

    /// 全部交互
    Full,
}

/// 搜索记录方式
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchQueryMode {
    /// 保留原文
    Raw,

    /// 仅存散列
    Hashed,

    /// 不记录搜索
    Off,
}

/// 保留天数；false 为永久
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetentionDays {
    /// 永久保留
    Forever,

    /// 最近 N 天
    Days(u32),
}

impl<'de> Deserialize<'de> for RetentionDays {
    /// 接受 Lua `false`(→ Forever)或正整数(→ Days);`true` / `0` / 负数报落型错。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(RetentionVisitor)
    }
}

/// `RetentionDays` 反序列化访问器:容忍 bool(false)与正整数两种形态。
struct RetentionVisitor;

impl serde::de::Visitor<'_> for RetentionVisitor {
    type Value = RetentionDays;

    /// 期望形态描述(serde 错误信息用)。
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("false(永久)或正整数天数")
    }

    /// `false` → 永久;`true` 无意义,报错。
    fn visit_bool<E>(self, value: bool) -> Result<RetentionDays, E>
    where
        E: serde::de::Error,
    {
        if value {
            Err(E::custom("retention_days 只接受 false(永久),true 无意义"))
        } else {
            Ok(RetentionDays::Forever)
        }
    }

    /// 无符号整数天数。
    fn visit_u64<E>(self, value: u64) -> Result<RetentionDays, E>
    where
        E: serde::de::Error,
    {
        days_from(value.try_into().ok())
    }

    /// 有符号整数天数(Lua 5.4 integer 走这条)。
    fn visit_i64<E>(self, value: i64) -> Result<RetentionDays, E>
    where
        E: serde::de::Error,
    {
        days_from(u32::try_from(value).ok())
    }
}

/// 把可选天数收敛成 `RetentionDays`:`None` / `0` 报错(要永久用 `false`)。
fn days_from<E>(days: Option<u32>) -> Result<RetentionDays, E>
where
    E: serde::de::Error,
{
    match days {
        Some(0) | None => Err(E::custom("retention_days 不能为 0(要永久请用 false)")),
        Some(n) => Ok(RetentionDays::Days(n)),
    }
}

/// 报告计算口径，不影响采集
#[config_section]
pub struct ReportConfig {
    /// 计入榜单和完播率的最短收听时长
    min_listen_secs: u64,

    /// 榜单条数，CLI --top 可覆盖
    top_limit: usize,
}

/// 行为统计
#[config_section]
pub struct StatsConfig {
    /// 采集范围
    level: StatsLevel,

    /// 按事件开关采集，不能覆盖 plays 和 sessions
    #[serde(deserialize_with = "deserialize_collect")]
    collect: FxHashMap<String, bool>,

    /// 搜索记录方式
    search_queries: SearchQueryMode,

    /// 不采集的来源
    #[serde(deserialize_with = "deserialize_source_list")]
    #[lua_type("mineral.SourceName[]")]
    exclude_sources: Vec<String>,

    /// 划分新会话的播放间隔
    session_gap_minutes: u64,

    /// 保留天数；false 为永久
    retention_days: RetentionDays,

    /// 报告计算口径
    report: ReportConfig,
}

/// 将不采集的来源列表落型;Lua 空表视为空列表,其他非数组值报错。
fn deserialize_source_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                serde_json::Value::String(s) => Ok(s),
                other => Err(serde::de::Error::custom(format!(
                    "列表元素须为字符串,得到 `{other}`"
                ))),
            })
            .collect(),
        serde_json::Value::Object(map) if map.is_empty() => Ok(Vec::new()),
        other => Err(serde::de::Error::custom(format!(
            "期望字符串数组(空表亦可),得到 `{other}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{RetentionDays, SearchQueryMode, StatsLevel};

    /// 经 serde_json 模拟落型:字符串枚举小写解析。
    #[test]
    fn level_and_mode_parse_lowercase() -> color_eyre::Result<()> {
        assert_eq!(
            serde_json::from_str::<StatsLevel>("\"full\"")?,
            StatsLevel::Full
        );
        assert_eq!(
            serde_json::from_str::<StatsLevel>("\"off\"")?,
            StatsLevel::Off
        );
        assert_eq!(
            serde_json::from_str::<SearchQueryMode>("\"hashed\"")?,
            SearchQueryMode::Hashed
        );
        Ok(())
    }

    #[test]
    fn retention_false_is_forever() -> color_eyre::Result<()> {
        assert_eq!(
            serde_json::from_str::<RetentionDays>("false")?,
            RetentionDays::Forever
        );
        Ok(())
    }

    #[test]
    fn retention_positive_is_days() -> color_eyre::Result<()> {
        assert_eq!(
            serde_json::from_str::<RetentionDays>("365")?,
            RetentionDays::Days(365)
        );
        Ok(())
    }

    #[test]
    fn retention_zero_and_true_rejected() {
        assert!(serde_json::from_str::<RetentionDays>("0").is_err());
        assert!(serde_json::from_str::<RetentionDays>("true").is_err());
    }

    /// collect 校验:对 core 本体(plays / sessions)的覆盖落型即拒;普通 kind 键放行。
    #[test]
    fn collect_rejects_core_table_overrides() {
        let ok = super::deserialize_collect(&mut serde_json::Deserializer::from_str(
            r#"{"searches": false, "downloads": true}"#,
        ));
        assert!(ok.is_ok(), "普通事件 kind 键应放行");

        for bad in [r#"{"plays": false}"#, r#"{"sessions": true}"#] {
            let got = super::deserialize_collect(&mut serde_json::Deserializer::from_str(bad));
            assert!(got.is_err(), "core 本体覆盖应拒:{bad}");
        }
    }
}
