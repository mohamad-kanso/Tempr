//! Frame-time probe for the result grid (Phase 1 AC: 100,000 rows scroll at
//! 60 fps). Pure bookkeeping — the view drives it from `on_next_frame`
//! callbacks and feeds it frame timestamps; this module only does the math.

use std::time::{Duration, Instant};

/// A frame is counted as dropped when it exceeds the 60 Hz budget (16.67 ms)
/// by more than vsync jitter allows — 60 Hz frames land at 16.6–16.9 ms.
pub const DROP_THRESHOLD: Duration = Duration::from_millis(20);

pub struct ScrollBench {
    total_rows: usize,
    step: usize,
    next_row: usize,
    last_tick: Option<Instant>,
    frames: Vec<Duration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BenchReport {
    pub frames: usize,
    pub min: Duration,
    pub avg: Duration,
    pub p95: Duration,
    pub max: Duration,
    /// Frames slower than [`DROP_THRESHOLD`].
    pub dropped: usize,
}

impl ScrollBench {
    /// Scroll through `total_rows` in roughly `target_frames` steps.
    pub fn new(total_rows: usize, target_frames: usize) -> Self {
        let step = (total_rows / target_frames.max(1)).max(1);
        Self {
            total_rows,
            step,
            next_row: 0,
            last_tick: None,
            frames: Vec::with_capacity(target_frames + 1),
        }
    }

    pub fn step(&self) -> usize {
        self.step
    }

    pub fn frames_recorded(&self) -> usize {
        self.frames.len()
    }

    /// Record a frame boundary at `now` and return the row to scroll to next,
    /// or `None` when the sweep is complete. The first call only arms the
    /// clock (no sample), so setup cost is not counted.
    pub fn tick(&mut self, now: Instant) -> Option<usize> {
        if let Some(last) = self.last_tick.replace(now) {
            self.frames.push(now.saturating_duration_since(last));
        }
        if self.next_row >= self.total_rows {
            return None;
        }
        let row = self.next_row;
        self.next_row = self.next_row.saturating_add(self.step);
        Some(row)
    }

    pub fn report(&self) -> BenchReport {
        let n = self.frames.len();
        if n == 0 {
            return BenchReport {
                frames: 0,
                min: Duration::ZERO,
                avg: Duration::ZERO,
                p95: Duration::ZERO,
                max: Duration::ZERO,
                dropped: 0,
            };
        }
        let mut sorted = self.frames.clone();
        sorted.sort();
        let total: Duration = sorted.iter().sum();
        let p95_ix = ((n as f64 * 0.95).ceil() as usize).clamp(1, n) - 1;
        BenchReport {
            frames: n,
            min: sorted[0],
            avg: total / n as u32,
            p95: sorted[p95_ix],
            max: sorted[n - 1],
            dropped: sorted.iter().filter(|d| **d > DROP_THRESHOLD).count(),
        }
    }
}

impl BenchReport {
    pub fn summary(&self) -> String {
        let fps = if self.avg.is_zero() {
            0.0
        } else {
            1.0 / self.avg.as_secs_f64()
        };
        format!(
            "scroll bench: {} frames · {:.1} fps · min {:.1} · avg {:.1} · p95 {:.1} · max {:.1} ms · {} dropped (>{} ms)",
            self.frames,
            fps,
            self.min.as_secs_f64() * 1e3,
            self.avg.as_secs_f64() * 1e3,
            self.p95.as_secs_f64() * 1e3,
            self.max.as_secs_f64() * 1e3,
            self.dropped,
            DROP_THRESHOLD.as_millis()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweeps_rows_in_steps_then_ends() {
        let mut b = ScrollBench::new(10, 4);
        assert_eq!(b.step(), 2);
        let t0 = Instant::now();
        let rows: Vec<Option<usize>> = (0..7)
            .map(|i| b.tick(t0 + Duration::from_millis(16 * i)))
            .collect();
        assert_eq!(
            rows,
            vec![Some(0), Some(2), Some(4), Some(6), Some(8), None, None]
        );
        // 7 ticks → 6 intervals; the first tick arms the clock.
        assert_eq!(b.report().frames, 6);
    }

    #[test]
    fn report_stats() {
        let mut b = ScrollBench::new(100, 5);
        let t0 = Instant::now();
        let mut t = t0;
        b.tick(t);
        for ms in [10u64, 12, 15, 30, 16] {
            t += Duration::from_millis(ms);
            b.tick(t);
        }
        let r = b.report();
        assert_eq!(r.frames, 5);
        assert_eq!(r.max, Duration::from_millis(30));
        assert_eq!(r.p95, Duration::from_millis(30));
        assert_eq!(r.min, Duration::from_millis(10));
        assert_eq!(r.dropped, 1);
        assert!(r.summary().contains("5 frames"));
    }

    #[test]
    fn empty_report_is_zeroed() {
        let b = ScrollBench::new(0, 10);
        assert_eq!(b.report().frames, 0);
        assert_eq!(b.report().dropped, 0);
    }
}
