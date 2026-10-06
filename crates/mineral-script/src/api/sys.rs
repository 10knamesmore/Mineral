//! `mineral.sys`:host 独有的系统信息(脚本自己拿不到 / 拿不可靠的)。
//!
//! 全部是**常量字段**(运行期不变,加载时灌一次):`os` / `arch` /
//! `hostname` / `version` / `paths`。时间日期**不**在这里——Lua 标准库
//! `os.date("*t")` / `os.time()` 已是实时 + 结构化,不做重复 API。
//! 信息来自实际安装模块的进程，daemon 与 TUI 分别安装。
//! 不提供 `cwd`；文件操作使用 `paths.*`。

use mlua::{Lua, Table};

/// 把 `sys` 子表挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: daemon 或 TUI 的宿主模块表
pub(crate) fn install(lua: &Lua, mineral: &Table) -> mlua::Result<()> {
    let sys = lua.create_table()?;
    // 应用展示名(外部上报 / User-Agent / 通知标题拼串用)。
    sys.set("name", "Mineral")?;
    // std::env::consts::OS:Linux 为 "linux"、macOS 为 "macos"。
    sys.set("os", std::env::consts::OS)?;
    sys.set("arch", std::env::consts::ARCH)?;
    if let Ok(name) = nix::unistd::gethostname() {
        sys.set("hostname", name.to_string_lossy())?;
    }
    let version = lua.create_table()?;
    version.set("major", component(env!("CARGO_PKG_VERSION_MAJOR"))?)?;
    version.set("minor", component(env!("CARGO_PKG_VERSION_MINOR"))?)?;
    version.set("patch", component(env!("CARGO_PKG_VERSION_PATCH"))?)?;
    // `v:str()` 拼回 "x.y.z"(日志 / toast 拼串用;编译期定值,直接闭包捕获)。
    version.set(
        "str",
        lua.create_function(|_, ()| Ok(env!("CARGO_PKG_VERSION")))?,
    )?;
    sys.set("version", version)?;
    sys.set("paths", paths_table(lua)?)?;
    mineral.set("sys", sys)
}

/// 组装 `sys.paths` 子表。单项解析失败(HOME 缺失等极端环境)该字段
/// 缺席为 nil,不拖垮整个 API 安装。
fn paths_table(lua: &Lua) -> mlua::Result<Table> {
    let paths = lua.create_table()?;
    set_path(&paths, "config", mineral_paths::config_dir())?;
    set_path(&paths, "data", mineral_paths::data_dir())?;
    set_path(&paths, "cache", mineral_paths::cache_dir())?;
    set_path(
        &paths,
        "log",
        mineral_paths::cache_dir().map(|d| d.join("mineral.log")),
    )?;
    set_path(&paths, "socket", mineral_paths::socket_path())?;
    Ok(paths)
}

/// 解析成功才落字段;失败记 warn(极端环境可观测)。
fn set_path(
    paths: &Table,
    key: &str,
    value: mineral_paths::Result<std::path::PathBuf>,
) -> mlua::Result<()> {
    match value {
        Ok(p) => paths.set(key, p.display().to_string()),
        Err(e) => {
            mineral_log::warn!(
                target: "script",
                key,
                error = mineral_log::chain(&e),
                "sys.paths 单项解析失败,字段缺席"
            );
            Ok(())
        }
    }
}

/// 将 Cargo 注入的版本号分量解析为 Lua 整数，保留解析错误。
fn component(raw: &'static str) -> mlua::Result<i64> {
    raw.parse().map_err(mlua::Error::external)
}

#[cfg(test)]
mod tests {
    use crate::api::test_support::vm_with_host;

    /// `mineral.sys` 字段与编译环境一致:os/arch 来自 std consts,
    /// version 是结构化三分量(与 workspace 版本同步),hostname 非空。
    #[test]
    fn sys_exposes_os_arch_hostname_and_structured_version() -> color_eyre::Result<()> {
        let (lua, _host) = vm_with_host()?;
        let script = format!(
            r#"
            local mineral = require("mineral.daemon")
            assert(mineral.sys.name == "Mineral", "应用名应为 Mineral")
            assert(mineral.sys.os == "{os}", "os 应为编译目标")
            assert(mineral.sys.arch == "{arch}", "arch 应为编译目标")
            assert(type(mineral.sys.hostname) == "string" and #mineral.sys.hostname > 0,
                "hostname 应为非空字符串")
            local v = mineral.sys.version
            assert(v.major == {major} and v.minor == {minor} and v.patch == {patch},
                "version 应为结构化三分量")
            assert(v:str() == ("%d.%d.%d"):format(v.major, v.minor, v.patch),
                "v:str() 应拼回 x.y.z")
            "#,
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            major = env!("CARGO_PKG_VERSION_MAJOR"),
            minor = env!("CARGO_PKG_VERSION_MINOR"),
            patch = env!("CARGO_PKG_VERSION_PATCH"),
        );
        lua.load(&script).exec()?;
        Ok(())
    }

    /// `sys.paths` 五项与 mineral-paths 解析一致(socket 创建目录,宽断言存在即可)。
    #[test]
    fn sys_paths_match_mineral_paths() -> color_eyre::Result<()> {
        let (lua, _host) = vm_with_host()?;
        let script = format!(
            r#"
            local mineral = require("mineral.daemon")
            local p = mineral.sys.paths
            assert(p.config == "{config}", "config 路径不一致")
            assert(p.data == "{data}", "data 路径不一致")
            assert(p.cache == "{cache}", "cache 路径不一致")
            assert(p.log == "{cache}/mineral.log", "log 路径不一致")
            assert(type(p.socket) == "string" and #p.socket > 0, "socket 路径应存在")
            "#,
            config = mineral_paths::config_dir()?.display(),
            data = mineral_paths::data_dir()?.display(),
            cache = mineral_paths::cache_dir()?.display(),
        );
        lua.load(&script).exec()?;
        Ok(())
    }
}
