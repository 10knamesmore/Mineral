//! Converts declared HLS durations and exact fragment intervals into track time.
//! Fetching missing fragment headers remains the segment store's responsibility.

use std::ops::Range;
use std::time::Duration;

use super::super::error::{Error, Result};

/// HLS estimates choose seek candidates; learned sample intervals must form one
/// contiguous decode timeline starting at tick zero.
pub(super) struct Timeline {
    /// Track ticks per second, taken from the audio initialization.
    pub timescale: u32,

    pub declared_duration: Duration,

    estimated_starts: Vec<Duration>,

    /// Exact intervals learned from prepared fragments or lazily fetched headers.
    intervals: Vec<Option<Range<u64>>>,
}

impl Timeline {
    pub(super) fn new(
        durations: impl IntoIterator<Item = Duration>,
        timescale: u32,
    ) -> Result<Self> {
        let mut declared_duration = Duration::ZERO;
        let mut estimated_starts = Vec::new();
        for duration in durations {
            estimated_starts.push(declared_duration);
            declared_duration = declared_duration
                .checked_add(duration)
                .ok_or(Error::Overflow)?;
        }
        Ok(Self {
            timescale,
            declared_duration,
            intervals: vec![None; estimated_starts.len()],
            estimated_starts,
        })
    }

    pub(super) fn duration(&self, ticks: u64) -> Duration {
        let scale = u64::from(self.timescale);
        Duration::from_secs(ticks / scale)
            + Duration::from_nanos((ticks % scale) * 1_000_000_000 / scale)
    }

    pub(super) fn estimated_segment(&self, ticks: u64) -> usize {
        let time = self.duration(ticks);
        self.estimated_starts
            .partition_point(|start| *start <= time)
            .saturating_sub(1)
    }

    pub(super) fn startup_count(&self, target: Duration) -> usize {
        self.estimated_starts
            .partition_point(|start| *start < target)
    }

    pub(super) fn exact_duration_ticks(&self) -> Option<u64> {
        self.intervals
            .last()
            .and_then(Option::as_ref)
            .map(|timeline| timeline.end)
    }

    pub(super) fn known(&self, index: usize) -> Result<Option<Range<u64>>> {
        self.intervals
            .get(index)
            .cloned()
            .ok_or(Error::InvalidMedia {
                field: "segment timeline index",
            })
    }

    pub(super) fn prepared_prefix_end(&self, count: usize) -> Duration {
        self.intervals
            .iter()
            .take(count)
            .next_back()
            .and_then(Option::as_ref)
            .map_or(Duration::ZERO, |timeline| self.duration(timeline.end))
    }

    pub(super) fn record(&mut self, index: usize, range: Range<u64>) -> Result<()> {
        let previous_end = index
            .checked_sub(1)
            .and_then(|previous| self.intervals.get(previous))
            .and_then(Option::as_ref)
            .map(|previous| previous.end);
        let next_start = self
            .intervals
            .get(index + 1)
            .and_then(Option::as_ref)
            .map(|next| next.start);
        let current = self.intervals.get_mut(index).ok_or(Error::InvalidMedia {
            field: "segment timeline index",
        })?;
        if (index == 0 && range.start != 0)
            || previous_end.is_some_and(|end| end != range.start)
            || next_start.is_some_and(|start| start != range.end)
            || current.as_ref().is_some_and(|known| *known != range)
        {
            return Err(Error::Unsupported {
                layout: "non-contiguous decode timeline",
            });
        }
        *current = Some(range);
        Ok(())
    }
}
