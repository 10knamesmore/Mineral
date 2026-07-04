//! 解码 / 下载策略判定(纯函数):打开解码器的 seekable 抉择、交给解码器的 byte_len、
//! capture 是否真下完整段。抽成纯函数便于脱离音频设备单测。

use mineral_model::StreamLayout;

/// 解码器是否以 seekable 打开:本地文件 / 整块远端流(Contiguous)可全扫建 seek 索引(本地走磁盘
/// 快、整块流 open 廉价);分片远端流(Chunked)全扫要在网络上把整段拉一遍,故非 seekable 打开
/// (open 在第一个分片即返回,秒起播;代价是流式期间不支持向后 seek)。
///
/// Opt-4 升级接缝:分片远端流下完(`download_complete`)后,可从落盘 capture 以 seekable 重开解码器、
/// 对齐当前位置,恢复流式期间被牺牲的向后 seek —— 那次升级的判定入口就是本函数。
///
/// # Params:
///   - `local`: 是否本地文件源
///   - `layout`: 流的容器布局
///
/// # Return:
///   `true` = seekable 打开;`false` = 流式打开(不预扫)。
fn open_seekable(local: bool, layout: StreamLayout) -> bool {
    local || layout == StreamLayout::Contiguous
}

/// 实际交给解码器的 byte_len:仅当 [`open_seekable`] 为真时给(→ 解码器 seekable、建全 seek 索引);
/// 分片远端流丢弃 byte_len(→ 非 seekable,open 不预扫全片)。
///
/// # Params:
///   - `byte_len`: 取流 / 本地拿到的字节长度(可能无 `Content-Length`)
///   - `local`: 是否本地文件源
///   - `layout`: 流的容器布局
///
/// # Return:
///   seekable 打开时原样返回 `byte_len`,否则 `None`。
pub(crate) fn effective_byte_len(
    byte_len: Option<u64>,
    local: bool,
    layout: StreamLayout,
) -> Option<u64> {
    if open_seekable(local, layout) {
        byte_len
    } else {
        None
    }
}

/// capture 是否真下完整段:期望字节数已知(`Some(>0)`)时要求文件不小于它。
///
/// 关键:stream_download 在下载**出错 / 断连**时也会 signal complete(把 `Err` 当 `Completed`
/// 处理),`wait_for_completion` 照样返回。若不核对字节数,截断文件会被标「下完」并 harvest 进缓存,
/// 之后每次播这个截断缓存都解码 IO 错。期望未知(`None` / `0`,无 `Content-Length`)时无从核对,
/// 退回信任已完成。
///
/// # Params:
///   - `file_len`: capture 落盘文件的实际字节数
///   - `expected`: 期望总字节(HTTP `Content-Length`)
///
/// # Return:
///   `true` = 真下完(可 harvest);`false` = 截断,不该入缓存。
pub(crate) fn download_reached_full(file_len: u64, expected: Option<u64>) -> bool {
    match expected {
        Some(total) if total > 0 => file_len >= total,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::StreamLayout;

    use super::{download_reached_full, effective_byte_len, open_seekable};

    /// open_seekable 决策:分片远端流(Chunked)以**非** seekable 打开(否则 symphonia open 时
    /// 扫遍所有分片建索引、网络流等于拉整段致起播慢);整块远端流(Contiguous)与本地文件照常
    /// seekable(本地全扫走磁盘,快)。
    #[test]
    fn chunked_remote_opens_non_seekable() {
        assert!(
            !open_seekable(/*local*/ false, StreamLayout::Chunked),
            "分片远端流非 seekable 打开"
        );
        assert!(
            open_seekable(/*local*/ false, StreamLayout::Contiguous),
            "整块远端流 seekable"
        );
        assert!(
            open_seekable(/*local*/ true, StreamLayout::Chunked),
            "本地分片文件仍 seekable(磁盘全扫快)"
        );
    }

    /// effective_byte_len:仅在以 seekable 打开时把 byte_len 交给解码器(→ 建全 seek 索引);
    /// 分片远端流丢弃 byte_len(→ 解码器非 seekable、开播不预扫)。
    #[test]
    fn effective_byte_len_drops_len_for_chunked_remote() {
        assert_eq!(
            effective_byte_len(Some(100), /*local*/ false, StreamLayout::Chunked),
            None,
            "分片远端流丢 byte_len"
        );
        assert_eq!(
            effective_byte_len(Some(100), /*local*/ false, StreamLayout::Contiguous),
            Some(100),
            "整块远端流保留"
        );
        assert_eq!(
            effective_byte_len(Some(100), /*local*/ true, StreamLayout::Chunked),
            Some(100),
            "本地保留"
        );
    }

    /// download_reached_full:capture 字节数达 content_length 才算真下完。回归:stream_download
    /// 在下载出错/断连时也 signal complete,若不核对字节数,截断文件会被标下完、harvest 进缓存,
    /// 之后每次播这个截断缓存都解码 IO 错(卡住)。
    #[test]
    fn download_reached_full_rejects_truncated() {
        assert!(
            !download_reached_full(84_000_000, Some(114_000_000)),
            "截断(缺 30MB)不算下完"
        );
        assert!(
            download_reached_full(114_000_000, Some(114_000_000)),
            "字节达标算下完"
        );
        assert!(
            download_reached_full(114_000_001, Some(114_000_000)),
            "略多于 content_length 也算(容错)"
        );
        assert!(
            download_reached_full(10, None),
            "无 Content-Length 无从核对,信任已完成"
        );
        assert!(
            download_reached_full(10, Some(0)),
            "Content-Length 为 0 无从核对,信任"
        );
    }
}
