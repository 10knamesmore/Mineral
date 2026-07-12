//! shelf 扫描:遍历 storage backend,产出「直接含音频文件的目录」列表(organize 的输入)。
//!
//! 遍历与探测解耦:扫描把文件事实(路径 + size/mtime + lofty 探测结果)收齐,
//! 「目录 → 歌单 / 专辑 / 艺人」的分组决策留给 organize(尚未接入)。

mod result;
mod walk;

pub use result::{ScannedDir, ScannedFile};
pub use walk::{ScanOptions, scan};
