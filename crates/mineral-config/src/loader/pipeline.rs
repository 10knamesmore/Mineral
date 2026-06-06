//! `load()` 主管线:eval default → eval user → 深合并 → 反序列化,永不因用户配置失败。

use std::path::Path;

use color_eyre::eyre::eyre;
use mlua::{Lua, Table, Value};

use crate::loader::merge::deep_merge;
use crate::loader::stub::inject_noop_host;
use crate::loader::warning::ConfigWarning;
use crate::schema::Config;

/// 加载用户配置。用户 `config.lua` 的任何错误降级为纯默认 + 一条 [`ConfigWarning`];
/// 仅当内置 `default.lua` 损坏(程序员错误,守卫测试拦截)才返回 `Err`。
///
/// 管线:内置 `default.lua` eval → 用户 `config.lua` eval(缺失则跳过)→ Lua 层深合并
/// → `serde_json` 中转 + `serde_path_to_error` 反序列化。非 daemon 进程已注入 no-op
/// host stub,故用户配置顶层 `mineral.on(...)` 安全。
///
/// # Params:
///   - `user_path`: 用户配置文件路径;不存在视为纯默认。
///
/// # Return:
///   `(Config, warnings)`:warnings 非空 = 用了默认兜底,调用方据此 toast。
pub fn load(user_path: &Path) -> color_eyre::Result<(Config, Vec<ConfigWarning>)> {
    let lua = new_vm()?;
    let (config, warnings, _user_evaled) = load_on(&lua, user_path)?;
    Ok((config, warnings))
}

/// daemon 专用:在**带活 host API** 的 VM 上加载 —— `config.lua` 顶层的
/// `mineral.on(...)` 等调用真实注册,成功后把 VM 交还调用方移交脚本运行时。
///
/// 配置与脚本是同一次 eval,**失败同沉**:用户文件缺失 / eval 失败 / 配置
/// 落型失败,一律回落默认配置且不交还 VM(`None`,部分注册过的 VM 弃掉)。
///
/// # Params:
///   - `user_path`: 用户配置文件路径;不存在视为纯默认(也无脚本可跑)。
///   - `install`: 把活 host API 挂进 VM 的回调(daemon 传脚本运行时的安装器;
///     本 crate 不依赖脚本 crate,方向由调用方注入)。
///
/// # Return:
///   `(Config, warnings, Option<Lua>)`:`Some(lua)` 仅当用户脚本 eval 且配置
///   落型全部成功。
pub fn load_with_vm(
    user_path: &Path,
    install: impl FnOnce(&Lua) -> color_eyre::Result<()>,
) -> color_eyre::Result<(Config, Vec<ConfigWarning>, Option<Lua>)> {
    let lua = Lua::new();
    install(&lua)?;
    let (config, warnings, user_evaled) = load_on(&lua, user_path)?;
    let vm = user_evaled.then_some(lua);
    Ok((config, warnings, vm))
}

/// `load` / `load_with_vm` 的共同主体:在给定 VM 上 eval default → eval user
/// → 深合并 → 落型,任何用户侧失败降级为默认 + warning。
///
/// # Params:
///   - `lua`: 已注入 host API(no-op 或活实现)的 VM
///   - `user_path`: 用户配置文件路径
///
/// # Return:
///   `(Config, warnings, user_evaled)`:`user_evaled` 为 true 表示用户文件
///   存在、eval 成功且配置落型成功(VM 内的脚本注册有效)。
fn load_on(lua: &Lua, user_path: &Path) -> color_eyre::Result<(Config, Vec<ConfigWarning>, bool)> {
    let default_table = eval_default(lua)?;
    let mut warnings = Vec::<ConfigWarning>::new();

    let user_table = match eval_user(lua, user_path) {
        Ok(table) => table,
        Err(warning) => {
            warnings.push(warning);
            None
        }
    };

    let Some(user) = user_table else {
        let (config, warnings) = finalize_default(default_table, warnings)?;
        return Ok((config, warnings, false));
    };

    let merged = deep_merge(lua, default_table.clone(), user)?;
    match from_lua_table(merged) {
        Ok(config) => Ok((config, warnings, true)),
        Err(warning) => {
            warnings.push(warning);
            let (config, warnings) = finalize_default(default_table, warnings)?;
            Ok((config, warnings, false))
        }
    }
}

impl Config {
    /// 纯默认配置(eval `default.lua`)。仅守卫测试与降级路径用;业务正常路径走 [`load`]。
    ///
    /// # Return:
    ///   内置默认;若 `default.lua` 自身坏(不该发生,有守卫测试)返回 `Err`。
    pub fn defaults() -> color_eyre::Result<Self> {
        let lua = new_vm()?;
        let table = eval_default(&lua)?;
        from_lua_table(table).map_err(|w| eyre!("default.lua 无法落成 Config:{w}"))
    }
}

/// 把默认表落成 `Config` 并打包 warnings;default 坏则 fail(程序员错误)。
///
/// # Params:
///   - `default_table`: 默认配置表
///   - `warnings`: 已累积的用户配置 warnings
///
/// # Return:
///   `(默认 Config, warnings)`
fn finalize_default(
    default_table: Table,
    warnings: Vec<ConfigWarning>,
) -> color_eyre::Result<(Config, Vec<ConfigWarning>)> {
    let config = from_lua_table(default_table)
        .map_err(|w| eyre!("default.lua 无法落成 Config(应被守卫测试拦截):{w}"))?;
    Ok((config, warnings))
}

/// 建 VM 并注入 no-op host stub。
///
/// # Return:
///   就绪的 VM
fn new_vm() -> color_eyre::Result<Lua> {
    let lua = Lua::new();
    inject_noop_host(&lua)?;
    Ok(lua)
}

/// eval 内置 `default.lua`,返回默认表(必成功,守卫测试守)。
///
/// # Params:
///   - `lua`: 目标 VM
///
/// # Return:
///   默认配置表
fn eval_default(lua: &Lua) -> color_eyre::Result<Table> {
    let table: Table = lua
        .load(include_str!("../lua/default.lua"))
        .set_name("default.lua")
        .eval()?;
    Ok(table)
}

/// eval 用户文件(若存在)。文件不存在 → `Ok(None)`;eval / 读取失败 → `ConfigWarning::Eval`。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `path`: 用户配置路径
///
/// # Return:
///   `Ok(Some(table))` 用户表 / `Ok(None)` 文件缺失 / `Err(warning)` eval 失败
fn eval_user(lua: &Lua, path: &Path) -> Result<Option<Table>, ConfigWarning> {
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ConfigWarning::Eval {
                detail: format!("读取 {} 失败:{e}", path.display()),
            });
        }
    };
    let table: Table =
        lua.load(&src)
            .set_name("config.lua")
            .eval()
            .map_err(|e| ConfigWarning::Eval {
                detail: format!("{e}"),
            })?;
    Ok(Some(table))
}

/// 合并表 → 强类型:`serde_json` 中转 + `serde_path_to_error` 拿精确字段路径。
///
/// # Params:
///   - `table`: 合并后的配置表
///
/// # Return:
///   `Config`,失败带字段路径
fn from_lua_table(table: Table) -> Result<Config, ConfigWarning> {
    let value = Value::Table(table);
    let json = serde_json::to_value(&value).map_err(|e| ConfigWarning::Deserialize {
        path: String::new(),
        detail: format!("Lua→JSON 转换失败:{e}"),
    })?;
    serde_path_to_error::deserialize::<_, Config>(&json).map_err(|e| ConfigWarning::Deserialize {
        path: e.path().to_string(),
        detail: e.inner().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mineral_model::BitRate;

    use super::load;
    use crate::loader::warning::ConfigWarning;
    use crate::schema::Config;

    /// 把内容写到唯一临时文件,返回路径;调用方测试结束自行删。
    fn temp_config(tag: &str, content: &str) -> color_eyre::Result<PathBuf> {
        let path =
            std::env::temp_dir().join(format!("mineral-cfg-test-{}-{tag}.lua", std::process::id()));
        std::fs::write(&path, content)?;
        Ok(path)
    }

    #[test]
    fn defaults_snapshot() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        mineral_test::assert_snap!("默认配置全量(default.lua → Config)", format!("{cfg:#?}"));
        Ok(())
    }

    #[test]
    fn absent_user_file_is_pure_default() -> color_eyre::Result<()> {
        let absent = std::env::temp_dir().join("mineral-cfg-does-not-exist-zzz.lua");
        let (cfg, warnings) = load(&absent)?;
        assert!(warnings.is_empty(), "缺文件不应产 warning");
        assert_eq!(*cfg.audio().volume(), 100);
        Ok(())
    }

    #[test]
    fn user_override_deep_merges() -> color_eyre::Result<()> {
        let path = temp_config("override", "return { audio = { volume = 50 } }")?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty());
        assert_eq!(*cfg.audio().volume(), 50, "覆盖生效");
        assert_eq!(*cfg.download().quality(), BitRate::Lossless, "其余仍默认");
        assert_eq!(*cfg.cache().audio_capacity(), 10 * 1024 * 1024 * 1024);
        Ok(())
    }

    #[test]
    fn bad_lua_falls_back_with_eval_warning() -> color_eyre::Result<()> {
        let path = temp_config("badlua", "this is not lua {{{")?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "回落默认");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Eval { .. }]),
            "应有一条 Eval warning,实得 {warnings:?}"
        );
        Ok(())
    }

    #[test]
    fn type_error_falls_back_with_field_path() -> color_eyre::Result<()> {
        let path = temp_config("typeerr", r#"return { audio = { volume = "loud" } }"#)?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "回落默认");
        match warnings.as_slice() {
            [ConfigWarning::Deserialize { path, .. }] => {
                assert_eq!(path, "audio.volume", "字段路径应精确");
            }
            other => {
                return Err(color_eyre::eyre::eyre!(
                    "应有一条 Deserialize warning:{other:?}"
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn unknown_field_rejected_with_path() -> color_eyre::Result<()> {
        let path = temp_config("unknown", "return { audio = { bogus = 1 } }")?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "回落默认");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Deserialize { .. }]),
            "未知字段应被 deny_unknown_fields 拒,实得 {warnings:?}"
        );
        Ok(())
    }

    #[test]
    fn load_with_vm_keeps_vm_and_live_api_works() -> color_eyre::Result<()> {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        // 活安装回调:挂一个计数函数,模拟 daemon 注入脚本运行时 API。
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in = Arc::clone(&calls);
        let path = temp_config(
            "livevm",
            "mineral.probe()\nreturn { audio = { volume = 60 } }",
        )?;
        let (cfg, warnings, vm) = super::load_with_vm(&path, |lua| {
            let mineral = lua.create_table()?;
            mineral.set(
                "probe",
                lua.create_function(move |_lua, ()| {
                    calls_in.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })?,
            )?;
            lua.globals().set("mineral", mineral)?;
            Ok(())
        })?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        assert_eq!(*cfg.audio().volume(), 60);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "活 API 必须真被调用");
        assert!(vm.is_some(), "eval 成功必须交还 VM");
        Ok(())
    }

    #[test]
    fn load_with_vm_absent_file_yields_no_vm() -> color_eyre::Result<()> {
        let absent = std::env::temp_dir().join("mineral-cfg-does-not-exist-vm.lua");
        let (cfg, warnings, vm) = super::load_with_vm(&absent, |_lua| Ok(()))?;
        assert!(warnings.is_empty());
        assert_eq!(*cfg.audio().volume(), 100);
        assert!(vm.is_none(), "无用户文件就无脚本,不交还 VM");
        Ok(())
    }

    #[test]
    fn load_with_vm_eval_failure_sinks_both() -> color_eyre::Result<()> {
        let path = temp_config("badvm", "syntax error {{{")?;
        let (cfg, warnings, vm) = super::load_with_vm(&path, |_lua| Ok(()))?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "配置回落默认");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Eval { .. }]),
            "实得 {warnings:?}"
        );
        assert!(vm.is_none(), "eval 失败配置与脚本同沉,弃 VM");
        Ok(())
    }

    #[test]
    fn top_level_host_call_is_noop() -> color_eyre::Result<()> {
        // 用户顶层调 host API(非 daemon 进程)应安全无错。
        let path = temp_config(
            "hostcall",
            "mineral.on('track_finished', function() end)\nreturn { audio = { volume = 70 } }",
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "no-op stub 应吞调用,实得 {warnings:?}");
        assert_eq!(*cfg.audio().volume(), 70);
        Ok(())
    }
}
