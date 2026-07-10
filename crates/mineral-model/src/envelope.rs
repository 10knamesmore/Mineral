//! 一首歌的全曲振幅包络(离线预计算,进度条波形渲染用)。

use serde::{Deserialize, Serialize};

/// 一首歌的全曲振幅包络。
///
/// 点值 0..=255,按**全曲峰值**归一——包络表达相对起伏,安静的曲目也有可读形状。
/// 点数由产出方决定,渲染端按显示宽度自行重采样,这里不固定长度。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// 时间轴均分桶的振幅峰值(全曲峰值归一到 0..=255)。
    pub points: Vec<u8>,

    /// 产出算法版本;读取方与当前版本不符时视同缺失、触发重算。
    pub version: u16,
}

#[cfg(test)]
mod tests {
    use crate::envelope::Envelope;

    /// 包络经 serde 往返不变:points / version 都要原样回来(IPC 与 db 存取的共同前提)。
    #[test]
    fn envelope_survives_serde_roundtrip() -> color_eyre::Result<()> {
        let env = Envelope {
            points: vec![0, 128, 255],
            version: 1,
        };
        let back = serde_json::from_str::<Envelope>(&serde_json::to_string(&env)?)?;
        assert_eq!(back, env);
        Ok(())
    }
}
