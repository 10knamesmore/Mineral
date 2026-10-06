//! 初始化协调边界:用户文件保护、旧元数据清理和实际 LuaLS 产物闭合。

use super::run_init;
use mineral_config::InitOutcome;

/// 初始化只创建两份用户配置,第二次保留用户修改并刷新分宿主默认参考。
#[test]
fn generates_independent_assets_without_overwriting_user_files() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let first = run_init(dir.path())?;
    for name in [
        "daemon.lua",
        "tui.lua",
        ".luarc.json",
        "daemon-default.lua",
        "tui-default.lua",
    ] {
        assert!(dir.path().join(name).is_file());
    }
    for name in [
        "daemon.lua",
        "tui.lua",
        "types.lua",
        "daemon-config.lua",
        "tui-config.lua",
    ] {
        assert!(dir.path().join("lua/meta").join(name).is_file());
    }
    assert!(!dir.path().join("config.lua").exists());
    assert!(!dir.path().join("default.lua").exists());
    assert!(
        mineral_server::config::load_daemon(&dir.path().join("daemon.lua"))?
            .1
            .is_empty()
    );
    assert!(
        mineral_tui::config::load_tui(&dir.path().join("tui.lua"))?
            .1
            .is_empty()
    );
    assert!(
        first
            .iter()
            .all(|item| matches!(item, InitOutcome::Written(_)))
    );

    let daemon = "return { audio = { volume = 83 } }";
    let tui = "return { heartbeat_secs = 42 }";
    std::fs::write(dir.path().join("daemon.lua"), daemon)?;
    std::fs::write(dir.path().join("tui.lua"), tui)?;
    std::fs::write(dir.path().join("daemon-default.lua"), "invalid")?;
    std::fs::write(dir.path().join("tui-default.lua"), "invalid")?;
    let obsolete = dir.path().join("lua/meta/obsolete.lua");
    std::fs::write(&obsolete, "---@class RemovedApi")?;
    let second = run_init(dir.path())?;
    assert!(!obsolete.exists());
    for (name, source) in [("daemon.lua", daemon), ("tui.lua", tui)] {
        assert_eq!(std::fs::read_to_string(dir.path().join(name))?, source);
        assert!(
            second
                .iter()
                .any(|item| matches!(item, InitOutcome::Skipped(path) if path.ends_with(name)))
        );
    }
    for name in ["daemon-default.lua", "tui-default.lua"] {
        assert!(
            second
                .iter()
                .any(|item| matches!(item, InitOutcome::Written(path) if path.ends_with(name)))
        );
    }
    Ok(())
}

/// config init 写出的所有定义必须引用闭合,且两个配置根都有对应 setup API。
#[test]
fn initialized_luals_types_are_reference_closed() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    run_init(dir.path())?;
    let mut assembled = String::new();
    for name in [
        "types.lua",
        "daemon.lua",
        "tui.lua",
        "daemon-config.lua",
        "tui-config.lua",
    ] {
        assembled.push_str(&std::fs::read_to_string(
            dir.path().join("lua/meta").join(name),
        )?);
        assembled.push('\n');
    }
    let defined = type_names(&assembled, &["---@class ", "---@alias "]);
    let referenced = reference_names(&assembled);
    let undefined = referenced
        .iter()
        .filter(|name| !defined.contains(*name))
        .collect::<Vec<_>>();
    assert!(undefined.is_empty(), "产物引用了未定义的类型:{undefined:?}");
    assert!(defined.contains("mineral.DaemonConfig"));
    assert!(defined.contains("mineral.TuiConfig"));
    assert!(defined.contains("mineral.DaemonApi"));
    assert!(defined.contains("mineral.TuiApi"));
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

/// 只检查 LuaCATS 标签中的类型引用;代码和 prose 中的 DEFER 等成员不是类型。
fn reference_names(text: &str) -> rustc_hash::FxHashSet<String> {
    let mut out = rustc_hash::FxHashSet::<String>::default();
    for line in text
        .lines()
        .filter(|line| line.trim_start().starts_with("---@"))
    {
        for (start, _) in line.match_indices("mineral.") {
            let rest = line.get(start + "mineral.".len()..).unwrap_or_default();
            let name = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>();
            if name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                out.insert(format!("mineral.{name}"));
            }
        }
    }
    out
}

/// 跨 crate 枚举(mineral-model)不能使用宿主的 lua_enum 宏,alias 由脚本层分发;
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
    let line = mineral_script::TYPES_META
        .lines()
        .find_map(|line| line.strip_prefix(&marker))
        .ok_or_else(|| eyre!("types.lua 缺 {name} 定义"))?;
    Ok(line
        .split('|')
        .map(|token| token.trim().to_owned())
        .collect())
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
