//! Runtime / IPC socket 目录解析与加固。
//!
//! socket 是**运行期对象**(非配置/状态),按 XDG 规范属于 `XDG_RUNTIME_DIR`。macOS 无此变量,
//! 与 tmux / kakoune / emacs 一致退到 `$TMPDIR`(`std::env::temp_dir()`)下按 uid 建私有目录。
//! daemon 与 client **共用** [`socket_path`] 这一处推导,保证两端绑定/连接到同一路径。

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use color_eyre::eyre::{WrapErr, bail};

/// `sun_path` 字段上限(含结尾 NUL):macOS 104 / Linux 108 字节。超限直接报错而非截断
/// (截断会绑到错误路径)。
#[cfg(target_os = "macos")]
const SUN_PATH_MAX: usize = 104;

/// `sun_path` 字段上限(含结尾 NUL):macOS 104 / Linux 108 字节。
#[cfg(target_os = "linux")]
const SUN_PATH_MAX: usize = 108;

/// IPC socket 文件名(固定)。
const SOCKET_FILE: &str = "mineral.sock";

/// 解析 runtime 目录(socket 落它下面)。**不创建**。优先级:
/// 1. `$MINERAL_SOCKET_DIR`(绝对)—— 显式覆盖,原样用(测试隔离 / 用户自定义);
/// 2. `$XDG_RUNTIME_DIR`(绝对、且已存在时属主为本人)→ `<它>/mineral`;
/// 3. `std::env::temp_dir()`(读 `$TMPDIR`,缺则 `/tmp`)→ `mineral-<uid>`(uid 隔离多用户)。
///
/// # Return:
///   解析得到的目录路径。
pub(crate) fn runtime_dir() -> color_eyre::Result<PathBuf> {
    if let Some(v) = std::env::var_os("MINERAL_SOCKET_DIR").filter(|v| !v.is_empty())
        && Path::new(&v).is_absolute()
    {
        return Ok(PathBuf::from(v));
    }
    if let Some(dir) = xdg_runtime_dir() {
        return Ok(dir);
    }
    let uid = nix::unistd::geteuid().as_raw();
    Ok(std::env::temp_dir().join(format!("mineral-{uid}")))
}

/// `$XDG_RUNTIME_DIR/mineral`:仅当其为绝对路径、且(若已存在)属主是当前用户;否则 `None`(退 tmp)。
/// 属主校验防共享环境下别人预建的目录被误用(参照 kakoune)。
///
/// # Return:
///   合规时 `Some(<XDG_RUNTIME_DIR>/mineral)`,否则 `None`。
fn xdg_runtime_dir() -> Option<PathBuf> {
    let v = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?;
    if !Path::new(&v).is_absolute() {
        return None;
    }
    if let Ok(md) = std::fs::metadata(&v)
        && md.uid() != nix::unistd::geteuid().as_raw()
    {
        return None;
    }
    Some(PathBuf::from(v).join("mineral"))
}

/// IPC unix socket 的完整路径(`<runtime_dir>/mineral.sock`)。daemon bind、client connect、
/// stale 检测全走这一处。
///
/// 与 [`runtime_dir`] 不同,本函数**会**创建 runtime 目录、把权限收紧到 `0700` 并校验属主,
/// 再检查 `sun_path` 长度,调用方拿到路径即可直接 bind / connect。
///
/// # Return:
///   `<runtime_dir>/mineral.sock` 的绝对路径;目录创建/属主校验失败或路径超长返回 `Err`。
pub(crate) fn socket_path() -> color_eyre::Result<PathBuf> {
    let dir = runtime_dir().wrap_err("解析 runtime 目录失败")?;
    std::fs::create_dir_all(&dir)
        .wrap_err_with(|| format!("创建 runtime 目录失败 {}", dir.display()))?;
    harden_dir(&dir)?;
    let sock = dir.join(SOCKET_FILE);
    let len = sock.as_os_str().as_bytes().len();
    if len >= SUN_PATH_MAX {
        bail!(
            "socket 路径过长({len} >= {SUN_PATH_MAX}),请用 $MINERAL_SOCKET_DIR 指一个更短的目录: {}",
            sock.display()
        );
    }
    Ok(sock)
}

/// 校验目录属主是当前用户,并把权限收紧到 `0700`(防共享 `/tmp` 上的 socket 劫持)。
///
/// # Params:
///   - `dir`: 已创建的 runtime 目录
///
/// # Return:
///   成功返回 `Ok(())`;属主非本人或权限设置失败返回 `Err`。
fn harden_dir(dir: &Path) -> color_eyre::Result<()> {
    let md = std::fs::metadata(dir)
        .wrap_err_with(|| format!("stat runtime 目录失败 {}", dir.display()))?;
    let me = nix::unistd::geteuid().as_raw();
    if md.uid() != me {
        bail!(
            "runtime 目录属主非本人(uid={} != {}),拒绝使用 {}",
            md.uid(),
            me,
            dir.display()
        );
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .wrap_err_with(|| format!("收紧 runtime 目录权限失败 {}", dir.display()))?;
    Ok(())
}
