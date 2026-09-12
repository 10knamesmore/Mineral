//! client 侧 PCM 近期窗口:有界、可辨认断续,不追已经过时的动画样本。

use std::collections::VecDeque;

use mineral_protocol::PcmChunk;

/// 有界 PCM 窗口。
pub(super) struct PcmWindow {
    /// 样本容量上限(超出丢最旧)。
    capacity: usize,

    /// 近期样本。
    samples: VecDeque<f32>,

    /// 当前播放代次(`None` = 尚未收到样本)。
    generation: Option<u64>,

    /// 上一块的结束流位置(样本数)。
    next_position: Option<u64>,

    /// 待处理的断续事实(换代 / 缺口)。
    discontinuity: bool,
}

impl PcmWindow {
    /// 构造空窗口。
    ///
    /// # Params:
    ///   - `capacity`: 样本容量上限
    #[must_use]
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            samples: VecDeque::new(),
            generation: None,
            next_position: None,
            discontinuity: false,
        }
    }

    /// 当前样本数。
    #[cfg(test)]
    #[must_use]
    fn len(&self) -> usize {
        self.samples.len()
    }

    /// 推入一块样本。
    ///
    /// 换代(起播 / seek / 切输入格式)清空窗口并标记断续;同代缺口只标记事实,
    /// 不混入旧流样本的前提由代次保证。
    ///
    /// # Params:
    ///   - `chunk`: daemon 推来的样本块(不夺所有权,调用方仍可保留)
    pub(super) fn push(&mut self, chunk: &PcmChunk) {
        let generation_changed = self.generation != Some(chunk.generation);
        if generation_changed {
            self.samples.clear();
            self.generation = Some(chunk.generation);
            self.discontinuity = true;
        }
        let position_gap = self
            .next_position
            .is_some_and(|expected| expected != chunk.position);
        if chunk.gap || position_gap {
            self.discontinuity = true;
        }
        self.samples.extend(chunk.samples.iter().copied());
        let overflow = self.samples.len().saturating_sub(self.capacity);
        if overflow > 0 {
            self.samples.drain(..overflow);
            self.discontinuity = true;
        }
        self.next_position = Some(
            chunk
                .position
                .saturating_add(u64::try_from(chunk.samples.len()).unwrap_or(u64::MAX)),
        );
    }

    /// 取走全部样本(消费式)。
    #[must_use]
    pub(super) fn drain(&mut self) -> Vec<f32> {
        self.samples.drain(..).collect()
    }

    /// 取走断续标记(取后清除)。
    #[must_use]
    pub(super) fn take_discontinuity(&mut self) -> bool {
        std::mem::take(&mut self.discontinuity)
    }
}

#[cfg(test)]
mod tests {
    use super::PcmWindow;
    use mineral_protocol::PcmChunk;

    /// 造一块样本。
    fn chunk(generation: u64, position: u64, gap: bool, samples: &[f32]) -> PcmChunk {
        PcmChunk {
            generation,
            position,
            gap,
            sample_rate: Some(44_100),
            samples: samples.to_vec(),
        }
    }

    /// 换代清空窗口并标断续。
    #[test]
    fn generation_change_clears_window() {
        let mut window = PcmWindow::new(/*capacity*/ 16);
        window.push(&chunk(1, 0, false, &[1.0, 2.0]));
        let _ = window.take_discontinuity();
        window.push(&chunk(2, 0, false, &[3.0]));
        assert_eq!(window.len(), 1, "换代应清空旧流样本");
        assert!(window.take_discontinuity(), "换代应标断续");
        assert_eq!(window.drain(), vec![3.0]);
    }

    /// 缺口标记与位置跳变都标断续。
    #[test]
    fn gaps_mark_discontinuity() {
        let mut window = PcmWindow::new(/*capacity*/ 16);
        window.push(&chunk(1, 0, false, &[1.0, 2.0]));
        let _ = window.take_discontinuity();
        window.push(&chunk(1, 5, false, &[3.0]));
        assert!(window.take_discontinuity(), "位置跳变应标断续");
        window.push(&chunk(1, 6, true, &[4.0]));
        assert!(window.take_discontinuity(), "gap 标记应标断续");
    }

    /// 超容量丢最旧并标断续。
    #[test]
    fn window_is_bounded() {
        let mut window = PcmWindow::new(/*capacity*/ 4);
        window.push(&chunk(1, 0, false, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        assert_eq!(window.len(), 4);
        assert_eq!(window.drain(), vec![3.0, 4.0, 5.0, 6.0]);
        assert!(window.take_discontinuity(), "溢出应标断续");
    }

    /// drain 是消费式;断续标记取后清除。
    #[test]
    fn drain_consumes_once() {
        let mut window = PcmWindow::new(/*capacity*/ 16);
        window.push(&chunk(1, 0, false, &[1.0]));
        assert_eq!(window.drain(), vec![1.0]);
        assert!(window.drain().is_empty());
        assert!(window.take_discontinuity());
        assert!(!window.take_discontinuity(), "标记取后应清除");
    }
}
