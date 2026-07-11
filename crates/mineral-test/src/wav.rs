//! 测试用 WAV fixture:纯 std 手写 RIFF 头 + 16-bit PCM,免外部工具依赖。
//!
//! 给需要「真实可解码音频文件」的测试用(包络离线解码、本地播放路径等);
//! 波形内容由调用方给交错样本,便于构造已知形状(渐强 / 恒幅 / 静音)。

/// 把 i16 交错样本写成标准 16-bit PCM WAV 文件。
///
/// # Params:
///   - `path`: 目标文件路径
///   - `samples`: 交错样本(帧内按声道顺序)
///   - `channels`: 声道数(≥1)
///   - `sample_rate`: 采样率(Hz)
///
/// # Return:
///   写入成功返回 `Ok(())`;样本总字节数超 u32(WAV 上限)或 IO 失败返回 `Err`。
pub fn write_wav(
    path: &std::path::Path,
    samples: &[i16],
    channels: u16,
    sample_rate: u32,
) -> color_eyre::Result<()> {
    let data_len = u32::try_from(samples.len().saturating_mul(2))?;
    let byte_rate = sample_rate
        .saturating_mul(u32::from(channels))
        .saturating_mul(2);
    let block_align = channels.saturating_mul(2);
    let mut out = Vec::with_capacity(44 + samples.len().saturating_mul(2));
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt 块长
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // 位深
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out)?;
    Ok(())
}
