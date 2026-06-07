//! `mineral.sys`:host 独有的系统信息(脚本自己拿不到 / 拿不可靠的)。
//!
//! 全部是**常量字段**(运行期不变,加载时灌一次):`os` / `arch` /
//! `hostname` / `version` / `paths`。时间日期**不**在这里——Lua 标准库
//! `os.date("*t")` / `os.time()` 已是实时 + 结构化,不做重复 API。
//! 有意不给 `cwd`:daemon 的 cwd 取决于谁拉起它(终端 / systemd),
//! 无稳定语义;文件操作用 `paths.*`,子进程工作目录用 `spawn` 的 `opts.cwd`。

use mlua::{Lua, Table};

/// 把 `sys` 子表挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
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
    version.set("major", component(env!("CARGO_PKG_VERSION_MAJOR")))?;
    version.set("minor", component(env!("CARGO_PKG_VERSION_MINOR")))?;
    version.set("patch", component(env!("CARGO_PKG_VERSION_PATCH")))?;
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
    value: color_eyre::Result<std::path::PathBuf>,
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

/// 编译期版本号分量 → 整数(cargo 注入的合法数字串,解析失败兜 0 只是形式)。
fn component(raw: &'static str) -> i64 {
    raw.parse().unwrap_or(0)
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

    /// meta stub 必须声明 sys 的全部字段(编辑器侧补全的守卫)。
    #[test]
    fn meta_stub_declares_sys_fields() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        for needle in [
            "---@class mineral.sys",
            "---@field name \"Mineral\"",
            "---@field os \"linux\"|\"macos\"",
            "---@field arch string",
            "---@field hostname string",
            "---@field version mineral.SysVersion",
            "---@field paths mineral.SysPaths",
            "---@class mineral.SysVersion",
            "function SysVersion:str() end",
            "---@class mineral.SysPaths",
            "---@field config string",
            "---@field socket string",
        ] {
            assert!(meta.contains(needle), "meta stub 缺少:`{needle}`");
        }
        Ok(())
    }
}
