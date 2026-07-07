//! `config check`:加载 + 校验配置,渲染诊断文本(纯函数,供快照)。

use std::path::Path;

use mineral_model::BitRate;

use crate::loader::ConfigWarning;
use crate::schema::{BackendKind, Config};

/// 渲染配置诊断:有效配置摘要 + warnings(各带 file:line / 字段路径)。纯函数,供快照。
///
/// # Params:
///   - `config`: 已加载(可能回落默认)的配置
///   - `warnings`: 加载过程的非致命问题;非空表示用了默认兜底
///   - `default_download_dir`: `download.dir` 缺省时的真实回落目录(调用方解析好传入,
///     保持本函数纯净/快照确定;诊断据此把「默认」展示成真实路径而非占位)
///   - `color`: 是否输出 ANSI 颜色(非 tty / 测试传 `false`)
///
/// # Return:
///   多行诊断文本(无尾换行)
pub fn render_check(
    config: &Config,
    warnings: &[ConfigWarning],
    default_download_dir: &Path,
    color: bool,
) -> String {
    let mut lines = Vec::<String>::new();
    lines.push(paint("Mineral 配置诊断", "1;36", color));
    lines.push(format!("  音量: {}", config.audio().volume()));
    lines.push(format!(
        "  音频后端: {}",
        backend_name(config.audio().backend())
    ));
    lines.push(format!(
        "  音频缓存(字节): {}",
        config.cache().audio_capacity()
    ));
    lines.push(format!(
        "  封面缓存(字节): {}",
        config.tui().cover().cache().disk()
    ));
    lines.push(format!(
        "  下载音质: {}",
        quality_name(config.download().quality())
    ));
    lines.push(format!(
        "  下载目录: {}",
        dir_label(config, default_download_dir)
    ));
    lines.push(format!(
        "  网易云超时(秒): {}",
        config.sources().netease().timeout_secs()
    ));
    lines.push(format!(
        "  gapless 预取(ms): {}",
        config.daemon().gapless_prefetch_ms()
    ));

    if warnings.is_empty() {
        lines.push(paint("配置有效,无警告。", "32", color));
    } else {
        lines.push(paint(
            &format!("有 {} 条警告(已回落默认):", warnings.len()),
            "1;31",
            color,
        ));
        for warning in warnings {
            lines.push(paint(&format!("  - {warning}"), "31", color));
        }
    }
    lines.join("\n")
}

/// 下载目录标签:显式配置则显示该路径;`None` 则显示真实默认目录 + `(默认)` 标注。
///
/// # Params:
///   - `config`: 配置
///   - `default_download_dir`: `download.dir` 缺省时的真实回落目录
///
/// # Return:
///   下载目录的人类可读路径
fn dir_label(config: &Config, default_download_dir: &Path) -> String {
    match config.download().dir() {
        Some(path) => path.display().to_string(),
        None => format!("{} (默认)", default_download_dir.display()),
    }
}

/// 后端枚举 → 展示名。
///
/// # Params:
///   - `backend`: 后端选择
///
/// # Return:
///   展示名
fn backend_name(backend: &BackendKind) -> &'static str {
    match backend {
        BackendKind::Auto => "auto",
        BackendKind::Null => "null",
    }
}

/// 音质枚举 → 展示名(小写,与 serde / config 字符串一致)。
///
/// # Params:
///   - `quality`: 音质
///
/// # Return:
///   展示名
fn quality_name(quality: &BitRate) -> &'static str {
    match quality {
        BitRate::Standard => "standard",
        BitRate::Higher => "higher",
        BitRate::Exhigh => "exhigh",
        BitRate::Lossless => "lossless",
        BitRate::Hires => "hires",
    }
}

/// 按需给文本加 ANSI 颜色;`color = false` 时原样返回。
///
/// # Params:
///   - `text`: 文本
///   - `code`: ANSI SGR 参数(如 `"32"`)
///   - `color`: 是否上色
///
/// # Return:
///   带 / 不带 ANSI 的文本
fn paint(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::render_check;
    use crate::loader::ConfigWarning;
    use crate::schema::Config;

    /// 测试用固定默认下载目录(避免快照随机器 / 环境变化)。
    fn fixed_default_dir() -> &'static Path {
        Path::new("/home/user/Music/mineral")
    }

    #[test]
    fn renders_valid_config() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let out = render_check(&cfg, &[], fixed_default_dir(), /*color*/ false);
        mineral_test::assert_snap!("config check:默认有效配置(无警告,无色)", out);
        Ok(())
    }

    #[test]
    fn renders_with_warnings() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let warnings = vec![ConfigWarning::Deserialize {
            path: "audio.volume".to_owned(),
            detail: "invalid type: string \"loud\", expected u8".to_owned(),
        }];
        let out = render_check(&cfg, &warnings, fixed_default_dir(), /*color*/ false);
        mineral_test::assert_snap!("config check:含一条字段路径警告(已回落默认,无色)", out);
        Ok(())
    }
}
