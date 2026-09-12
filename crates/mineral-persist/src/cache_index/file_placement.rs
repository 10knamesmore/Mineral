//! 缓存文件命名去重、源文件移入与内存字节落盘。

use std::path::Path;

use color_eyre::eyre::WrapErr;

/// 为入库文件选不撞名的相对路径并把 `src` move 过去,返回 `(relpath, bytes)`。
/// 同 key 复用 `existing`(原地覆盖);否则 `<subdir>/<file_name>`,撞盘则追加 ` (N)`。
///
/// # Params:
///   - `root`: 文件根目录
///   - `subdir`: 分区子目录
///   - `file_name`: 期望文件名(含扩展名)
///   - `src`: 源文件路径
///   - `existing`: 同 key 旧 relpath(有则复用)
///
/// # Return:
///   `(相对路径, move 后文件字节数)`。
pub(super) fn place_file(
    root: &Path,
    subdir: &str,
    file_name: &str,
    src: &Path,
    existing: Option<String>,
) -> color_eyre::Result<(String, u64)> {
    let rel = match existing {
        Some(r) => r,
        None => dedup_rel(root, subdir, file_name),
    };
    let dst = root.join(&rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("创建缓存子目录失败 dir={}", parent.display()))?;
    }
    if std::fs::rename(src, &dst).is_err() {
        // 跨分区 rename 失败 → copy + 删源(copy 暴露真实错误)。
        std::fs::copy(src, &dst).wrap_err_with(|| {
            format!("copy 入库失败 src={} dst={}", src.display(), dst.display())
        })?;
        drop(std::fs::remove_file(src));
    }
    // 刚落盘就 stat 不到 = 真实 IO 异常,冒泡而非把 0 当字节数记进索引(容量核算会失真)。
    let bytes = std::fs::metadata(&dst)
        .map(|m| m.len())
        .wrap_err_with(|| format!("stat 入库文件失败 path={}", dst.display()))?;
    Ok((rel, bytes))
}

/// 把 `data` 写到不撞名的相对路径并返回 `(relpath, bytes)`。同 key 复用 `existing`(原地覆盖);
/// 否则 `<subdir>/<file_name>`,撞盘则追加 ` (N)`。
///
/// # Params:
///   - `root`: 文件根目录
///   - `subdir`: 分区子目录
///   - `file_name`: 期望文件名(含扩展名)
///   - `data`: 落盘字节
///   - `existing`: 同 key 旧 relpath(有则复用)
///
/// # Return:
///   `(相对路径, 字节数)`。
pub(super) fn write_bytes_file(
    root: &Path,
    subdir: &str,
    file_name: &str,
    data: &[u8],
    existing: Option<String>,
) -> color_eyre::Result<(String, u64)> {
    let rel = match existing {
        Some(r) => r,
        None => dedup_rel(root, subdir, file_name),
    };
    let dst = root.join(&rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("创建缓存子目录失败 dir={}", parent.display()))?;
    }
    std::fs::write(&dst, data)
        .wrap_err_with(|| format!("写缓存文件失败 path={}", dst.display()))?;
    Ok((rel, u64::try_from(data.len())?))
}

/// 为新 key 选一个不与磁盘现有文件相撞的相对路径:`<subdir>/<file_name>`,撞则追加 ` (N)`。
///
/// # Params:
///   - `root`: 根目录
///   - `subdir`: 分区子目录
///   - `file_name`: 期望文件名(含扩展名)
///
/// # Return:
///   相对 `root`、当前未被占用的路径(`/` 分隔)。
fn dedup_rel(root: &Path, subdir: &str, file_name: &str) -> String {
    let first = format!("{subdir}/{file_name}");
    if !root.join(&first).exists() {
        return first;
    }
    let p = Path::new(file_name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);
    let ext = p.extension().and_then(|s| s.to_str());
    for n in 2u32..=9999 {
        let name = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let rel = format!("{subdir}/{name}");
        if !root.join(&rel).exists() {
            return rel;
        }
    }
    first // 极端兜底(几乎不可能):覆盖 first,绝不 panic
}
