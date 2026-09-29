//! `config init`:生成用户配置模板 + `.luarc.json` + 分发 LuaCATS stub 与默认值参考。

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// 用户配置模板(首次生成;已存在则不覆盖)。
const CONFIG_TEMPLATE: &str = include_str!("lua/template.lua");

/// 内置默认配置(随版本覆盖到用户目录,**仅供参考**——程序 eval 的是编译进
/// 二进制的这份内容,不读用户目录的副本)。
const DEFAULT_LUA: &str = include_str!("lua/default.lua");

/// 内置 host API stub(随版本覆盖到用户目录,供 LSP)。
const META_MINERAL: &str = include_str!("lua/meta/mineral.lua");

/// `.luarc.json` 内容
const LUARC_JSON: &str = include_str!("lua/luarc.json");

/// 一个文件的生成结果。
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InitOutcome {
    /// 已写入该路径。
    Written(PathBuf),

    /// 已存在,跳过(不覆盖用户内容)。
    Skipped(PathBuf),
}

impl std::fmt::Display for InitOutcome {
    /// 单行展示(供 CLI 打印)。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Written(path) => write!(f, "wrote    {}", path.display()),
            Self::Skipped(path) => write!(f, "skipped  {} (exists)", path.display()),
        }
    }
}

/// 生成全部配置资产:用户配置模板、`.luarc.json`、LuaCATS stub(`<config_dir>/lua/meta/`)
/// 与默认值参考副本 `default.lua`。`config.lua` / `.luarc.json` 已存在则跳过(不覆盖);
/// meta stub 与 default.lua 随版本覆盖。
///
/// # Params:
///   - `config_dir`: 配置目录(通常 `config_dir()`)。
///
/// # Return:
///   写入 / 跳过的文件清单(供 CLI 打印)。
pub fn run_init(config_dir: &Path) -> Result<Vec<InitOutcome>> {
    std::fs::create_dir_all(config_dir).map_err(|source| Error::InitIo {
        operation: "创建配置目录",
        path: config_dir.to_path_buf(),
        source,
    })?;
    let mut outcomes = Vec::<InitOutcome>::new();

    outcomes.push(write_if_absent(
        &config_dir.join("config.lua"),
        CONFIG_TEMPLATE,
    )?);
    outcomes.push(write_if_absent(
        &config_dir.join(".luarc.json"),
        LUARC_JSON,
    )?);
    // 默认值参考:程序不读它,只为让用户就近查“每个字段默认是什么”。
    outcomes.push(overwrite(&config_dir.join("default.lua"), DEFAULT_LUA)?);

    let meta_dir = config_dir.join("lua").join("meta");
    std::fs::create_dir_all(&meta_dir).map_err(|source| Error::InitIo {
        operation: "创建 Lua 元数据目录",
        path: meta_dir.clone(),
        source,
    })?;
    outcomes.push(overwrite(&meta_dir.join("mineral.lua"), META_MINERAL)?);
    // Config 类型 stub 是 Rust schema 的投影,每次拼装最新产物覆盖。
    outcomes.push(overwrite(
        &meta_dir.join("config.lua"),
        &crate::lua_stub::meta_config_lua(),
    )?);

    Ok(outcomes)
}

/// 写文件,仅当不存在;存在则跳过(保护用户内容)。
///
/// # Params:
///   - `path`: 目标路径
///   - `content`: 写入内容
///
/// # Return:
///   写入 / 跳过结果
fn write_if_absent(path: &Path, content: &str) -> Result<InitOutcome> {
    if path.exists() {
        Ok(InitOutcome::Skipped(path.to_path_buf()))
    } else {
        std::fs::write(path, content).map_err(|source| Error::InitIo {
            operation: "写入配置资产",
            path: path.to_path_buf(),
            source,
        })?;
        Ok(InitOutcome::Written(path.to_path_buf()))
    }
}

/// 写文件(总是覆盖;用于程序分发的 stub)。
///
/// # Params:
///   - `path`: 目标路径
///   - `content`: 写入内容
///
/// # Return:
///   写入结果
fn overwrite(path: &Path, content: &str) -> Result<InitOutcome> {
    std::fs::write(path, content).map_err(|source| Error::InitIo {
        operation: "覆盖配置资产",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(InitOutcome::Written(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::{InitOutcome, run_init};

    /// 唯一临时目录(进程 id + tag 隔离,避免并发碰撞)。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mineral-init-test-{}-{tag}", std::process::id()))
    }

    #[test]
    fn generates_all_assets_then_skips_config() -> color_eyre::Result<()> {
        let dir = temp_dir("gen");
        let _ = std::fs::remove_dir_all(&dir);

        let first = run_init(&dir)?;
        assert!(dir.join("config.lua").is_file(), "config.lua 应生成");
        assert!(dir.join(".luarc.json").is_file(), ".luarc.json 应生成");
        assert!(dir.join("default.lua").is_file(), "default.lua 应生成");
        assert!(
            dir.join("lua/meta/mineral.lua").is_file(),
            "host stub 应生成"
        );
        assert!(
            first.iter().all(|o| matches!(o, InitOutcome::Written(_))),
            "首次全为 Written:{first:?}"
        );

        // 二次:config.lua / .luarc.json 跳过,meta stub 与 default.lua 参考仍覆盖。
        let second = run_init(&dir)?;
        assert!(
            second
                .iter()
                .any(|o| matches!(o, InitOutcome::Skipped(p) if p.ends_with("config.lua"))),
            "二次应跳过 config.lua:{second:?}"
        );
        assert!(
            second
                .iter()
                .any(|o| matches!(o, InitOutcome::Written(p) if p.ends_with("default.lua"))),
            "default.lua 参考应随版本覆盖:{second:?}"
        );

        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
