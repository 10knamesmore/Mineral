//! 扫描产出的中间结构(organize 的输入单元)。

use std::path::PathBuf;
use std::time::SystemTime;

use derive_getters::Getters;
use mineral_probe::ProbedAudio;

/// 一个音频文件的扫描事实:路径 + 增量信号 + 探测结果。
#[derive(Clone, Debug, Getters)]
pub struct ScannedFile {
    /// 文件路径(backend 命名空间下)。
    path: PathBuf,

    /// 字节大小(增量扫描 size 比对用;backend 未给为 `None`)。
    size: Option<u64>,

    /// 最后修改时间(增量扫描 mtime 比对用;backend 未给为 `None`)。
    mtime: Option<SystemTime>,

    /// 按内容探测出的音频属性与标签。
    probed: ProbedAudio,
}

impl ScannedFile {
    /// 构造一条扫描文件事实。
    ///
    /// # Params:
    ///   - `path`: 文件路径
    ///   - `size`: 字节大小(缺失为 `None`)
    ///   - `mtime`: 最后修改时间(缺失为 `None`)
    ///   - `probed`: 探测结果
    ///
    /// # Return:
    ///   扫描文件事实。
    pub fn new(
        path: PathBuf,
        size: Option<u64>,
        mtime: Option<SystemTime>,
        probed: ProbedAudio,
    ) -> Self {
        Self {
            path,
            size,
            mtime,
            probed,
        }
    }
}

/// 一个「直接含音频文件的目录」及其音频文件。
///
/// 这是 organize 的输入单元:每个这样的目录是候选歌单 / 专辑,`files` 只含**直接**子级的
/// (成功探测的)音频文件——嵌套子目录的音频归各自的 [`ScannedDir`],不上卷。
#[derive(Clone, Debug, Getters)]
pub struct ScannedDir {
    /// 目录路径。
    path: PathBuf,

    /// 该目录直接含的音频文件(仅成功探测的)。
    files: Vec<ScannedFile>,
}

impl ScannedDir {
    /// 构造一个扫描目录。
    ///
    /// # Params:
    ///   - `path`: 目录路径
    ///   - `files`: 直接含的音频文件(仅成功探测的)
    ///
    /// # Return:
    ///   扫描目录。
    pub fn new(path: PathBuf, files: Vec<ScannedFile>) -> Self {
        Self { path, files }
    }
}
