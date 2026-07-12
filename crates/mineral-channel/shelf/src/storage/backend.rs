//! storage backend 抽象:[`ShelfStorage`] trait 及其词表([`Entry`] / [`PlayTarget`] / [`ShelfReader`])。

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use derive_getters::Getters;

/// 目录项类型(文件 / 目录)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// 普通文件。
    File,

    /// 子目录。
    Dir,
}

/// storage 列目录得到的一项。
///
/// `size` / `mtime` 用 `Option`:目录通常无有意义 size,某些 backend 不给 mtime——
/// 缺失就让类型说出来,增量扫描(size+mtime 比对)据此显式回落全量重探,不拿 `0` 当哨兵。
#[derive(Clone, Debug, PartialEq, Eq, Getters)]
pub struct Entry {
    /// 该项在 backend 命名空间下的路径。
    path: PathBuf,

    /// 文件 / 目录。
    kind: EntryKind,

    /// 字节大小(目录 / 未知为 `None`)。
    size: Option<u64>,

    /// 最后修改时间(backend 不给为 `None`)。
    mtime: Option<SystemTime>,
}

impl Entry {
    /// 构造一项(backend 实现用)。
    ///
    /// # Params:
    ///   - `path`: backend 命名空间下的路径
    ///   - `kind`: 文件 / 目录
    ///   - `size`: 字节大小(缺失为 `None`)
    ///   - `mtime`: 最后修改时间(缺失为 `None`)
    ///
    /// # Return:
    ///   目录项。
    pub fn new(
        path: PathBuf,
        kind: EntryKind,
        size: Option<u64>,
        mtime: Option<SystemTime>,
    ) -> Self {
        Self {
            path,
            kind,
            size,
            mtime,
        }
    }
}

/// 同步可定位读取器——lofty 探测 / 封面抽取要求 `Read + Seek`(探测跑在 blocking task 里)。
///
/// blanket impl 覆盖一切 `Read + Seek + Send`,backend 的 `open` 直接 `Box` 具体读取器即可。
pub trait ShelfReader: Read + Seek + Send {}

impl<T: Read + Seek + Send> ShelfReader for T {}

/// shelf 的存储后端抽象。
///
/// # 职责
/// 把「怎么列目录 / 读字节」从 shelf 的索引 / organize / 扫描生命周期里隔离出来:
/// 上层全部 backend 无关,换后端只加实现。
///
/// # 实现方须知
/// - `list_dir` 只列**直接子项**(一层);递归 / 深度 / 排除由扫描器控制,不是 backend 的事。
/// - `open` 是**同步** `Read + Seek`:探测与封面抽取要求随机访问;实现方把阻塞 IO 交给
///   blocking 上下文(host 已在 blocking task 里调用)。
///
/// **播放解析不在此 trait**:fs 文件的可播形态就是 `MediaUrl::Local(path)`,channel 从索引
/// 拿到 path 直接构 `PlayUrl`,不必问 backend。远端 backend(path→直链 / 鉴权头 / 字节流,
/// 需 backend 特定解析)落地时再加对应方法,那时按真实实现定形状(字节流交接落在引擎的
/// open 音源接缝——引擎已会 `SourceStream`,不进 metadata model)。
#[async_trait]
pub trait ShelfStorage: Send + Sync {
    /// 列出 `path` 目录的直接子项(文件 + 子目录,不递归)。
    async fn list_dir(&self, path: &Path) -> color_eyre::Result<Vec<Entry>>;

    /// 打开 `path` 文件供同步随机访问(探测 / 封面抽取)。
    fn open(&self, path: &Path) -> color_eyre::Result<Box<dyn ShelfReader>>;
}
