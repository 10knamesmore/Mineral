//! 遍历 storage backend,产出「直接含音频文件的目录」列表。

use std::collections::VecDeque;
use std::path::Path;

use mineral_probe::{ProbedAudio, is_audio_ext, probe};
use regex::Regex;

use super::result::{ScannedDir, ScannedFile};
use crate::storage::{EntryKind, ShelfStorage};

/// 遍历控制选项。
pub struct ScanOptions {
    /// 遍历深度上限(防 symlink 环 / 超深树);root 自身为深度 0。
    max_depth: usize,

    /// 目录 / 文件名排除 pattern(命中即跳过该目录整棵子树 / 该文件)。
    exclude: Vec<Regex>,
}

impl ScanOptions {
    /// 构造遍历选项。
    ///
    /// # Params:
    ///   - `max_depth`: 深度上限(root 为 0)
    ///   - `exclude`: 名称排除 pattern(已编译)
    ///
    /// # Return:
    ///   遍历选项。
    pub fn new(max_depth: usize, exclude: Vec<Regex>) -> Self {
        Self { max_depth, exclude }
    }

    /// 名称是否命中任一排除 pattern。
    ///
    /// # Params:
    ///   - `name`: 目录 / 文件名(不含路径)
    ///
    /// # Return:
    ///   命中返回 `true`。
    fn excluded(&self, name: &str) -> bool {
        self.exclude.iter().any(|re| re.is_match(name))
    }
}

/// 遍历 `root`,返回所有「直接含至少一个可探测音频文件」的目录。
///
/// 遍历经 storage backend(不直触 fs);音频文件按内容探测(经 mineral-probe),探测失败的
/// 文件 warn + 跳过,不拖垮整根扫描。名称命中 exclude 即跳过;深度超 `max_depth` 不再深入。
/// 只收**直接**含音频的目录,嵌套音频归各自子目录(「一个歌单 = 一个目录,只收第一层」)。
///
/// # Params:
///   - `storage`: 存储后端
///   - `root`: 遍历起点
///   - `opts`: 遍历控制
///
/// # Return:
///   含音频文件的目录列表(遍历序,不保证排序)。
pub async fn scan(storage: &dyn ShelfStorage, root: &Path, opts: &ScanOptions) -> Vec<ScannedDir> {
    let mut out = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back((root.to_owned(), 0_usize));

    while let Some((dir, depth)) = queue.pop_front() {
        let entries = match storage.list_dir(&dir).await {
            Ok(entries) => entries,
            Err(e) => {
                mineral_log::warn!(dir = %dir.display(), error = mineral_log::chain(&e), "列目录失败,跳过该目录");
                continue;
            }
        };

        let mut audio_files = Vec::new();
        for entry in entries {
            let Some(name) = entry.path().file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if opts.excluded(name) {
                continue;
            }
            match entry.kind() {
                EntryKind::Dir => {
                    if depth < opts.max_depth {
                        queue.push_back((entry.path().clone(), depth + 1));
                    }
                }
                EntryKind::File => {
                    let is_audio = entry
                        .path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(is_audio_ext);
                    if is_audio {
                        audio_files.push(entry);
                    }
                }
            }
        }

        let mut scanned = Vec::new();
        for file in audio_files {
            match probe_file(storage, file.path()).await {
                Some(probed) => scanned.push(ScannedFile::new(
                    file.path().clone(),
                    *file.size(),
                    *file.mtime(),
                    probed,
                )),
                None => {
                    mineral_log::warn!(file = %file.path().display(), "音频探测失败,跳过");
                }
            }
        }

        if !scanned.is_empty() {
            out.push(ScannedDir::new(dir, scanned));
        }
    }

    out
}

/// 经 backend 打开并按内容探测一个音频文件。
///
/// backend `open` 是同步的、探测读满文件是阻塞 IO,丢 blocking 池不堵 runtime worker。
///
/// # Params:
///   - `storage`: 存储后端
///   - `path`: 文件路径
///
/// # Return:
///   探测结果;打开 / 识别失败为 `None`。
async fn probe_file(storage: &dyn ShelfStorage, path: &Path) -> Option<ProbedAudio> {
    let reader = storage.open(path).ok()?;
    tokio::task::spawn_blocking(move || {
        let mut reader = reader;
        probe(&mut *reader)
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::{ScanOptions, scan};
    use crate::storage::FsStorage;

    /// 合法最小 WAV(44B 头 + `data_len` 个 0 PCM);写进目录当音频文件。
    fn wav_bytes(data_len: usize) -> Vec<u8> {
        let data = u32::try_from(data_len).unwrap_or(0);
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&36u32.saturating_add(data).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        v.resize(v.len() + data_len, 0u8);
        v
    }

    /// 默认排除 pattern(隐藏文件 / 目录:名字以点开头)。
    fn hide_dotfiles() -> color_eyre::Result<Vec<Regex>> {
        Ok(vec![Regex::new(r"^\.")?])
    }

    /// 「直接含音频的目录 = 一张歌单」:root 下两张专辑目录各出一个 ScannedDir,
    /// 非音频文件不计,root 本身无直接音频故不出 ScannedDir。
    #[tokio::test]
    async fn collects_dirs_directly_containing_audio() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        std::fs::create_dir(root.join("albumA"))?;
        std::fs::write(root.join("albumA").join("1.wav"), wav_bytes(2000))?;
        std::fs::write(root.join("albumA").join("2.wav"), wav_bytes(2000))?;
        std::fs::write(root.join("albumA").join("cover.jpg"), b"notaudio")?;
        std::fs::create_dir(root.join("albumB"))?;
        std::fs::write(root.join("albumB").join("x.wav"), wav_bytes(2000))?;

        let opts = ScanOptions::new(8, hide_dotfiles()?);
        let mut dirs = scan(&FsStorage, root, &opts).await;
        dirs.sort_by(|a, b| a.path().cmp(b.path()));

        assert_eq!(dirs.len(), 2, "两张专辑目录各成一个 ScannedDir,root 自身无直接音频不出");
        let names = dirs
            .iter()
            .map(|d| {
                d.path()
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect::<Vec<String>>();
        assert_eq!(names, vec!["albumA".to_owned(), "albumB".to_owned()]);
        let album_a = dirs
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 albumA"))?;
        assert_eq!(album_a.files().len(), 2, "albumA 两个音频,cover.jpg 不计");
        Ok(())
    }

    /// 探测出的属性带上:WAV → format Wav、有位深(探测结果确实进了 ScannedFile)。
    #[tokio::test]
    async fn scanned_file_carries_probe_result() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("s.wav"), wav_bytes(4000))?;

        let opts = ScanOptions::new(8, hide_dotfiles()?);
        let dirs = scan(&FsStorage, dir.path(), &opts).await;
        assert_eq!(dirs.len(), 1);
        let file = dirs
            .first()
            .and_then(|d| d.files().first())
            .ok_or_else(|| color_eyre::eyre::eyre!("应有一个扫描文件"))?;
        assert_eq!(
            file.probed().format(),
            &Some(mineral_model::AudioFormat::Wav)
        );
        assert!(file.size().is_some(), "size 带上(增量扫描要用)");
        Ok(())
    }

    /// 隐藏目录(`.git` 等)被 exclude 跳过,其中的音频不入库。
    #[tokio::test]
    async fn excludes_hidden_directories() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".hidden"))?;
        std::fs::write(dir.path().join(".hidden").join("secret.wav"), wav_bytes(2000))?;
        std::fs::write(dir.path().join("ok.wav"), wav_bytes(2000))?;

        let opts = ScanOptions::new(8, hide_dotfiles()?);
        let dirs = scan(&FsStorage, dir.path(), &opts).await;
        assert_eq!(dirs.len(), 1, "只有 root 自身(含 ok.wav),.hidden 被跳过");
        Ok(())
    }

    /// max_depth 限制深入:depth 0 = root,max_depth 1 时深度 2 的目录不再遍历。
    #[tokio::test]
    async fn max_depth_stops_recursion() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        // root/lvl1/lvl2/deep.wav —— deep 在深度 2。
        let deep = dir.path().join("lvl1").join("lvl2");
        std::fs::create_dir_all(&deep)?;
        std::fs::write(deep.join("deep.wav"), wav_bytes(2000))?;
        // root/lvl1/shallow.wav —— 在深度 1。
        std::fs::write(dir.path().join("lvl1").join("shallow.wav"), wav_bytes(2000))?;

        // max_depth 1:遍历到 lvl1(深度 1)止,不进 lvl2(深度 2)。
        let opts = ScanOptions::new(1, hide_dotfiles()?);
        let dirs = scan(&FsStorage, dir.path(), &opts).await;
        assert_eq!(dirs.len(), 1, "只出 lvl1(含 shallow.wav),lvl2 未遍历");
        let only = dirs
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 lvl1"))?;
        assert!(only.path().ends_with("lvl1"));
        Ok(())
    }
}
