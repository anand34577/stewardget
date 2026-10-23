//! The segment table: which byte ranges exist, who is working on them and how
//! far each has got. All connections of a download share one table behind a
//! mutex; every operation is O(segments) with tiny constants.
//!
//! ## Dynamic splitting
//!
//! When a connection runs out of work it does not sit idle: [`SegmentTable::claim`]
//! halves the active segment with the most bytes left and hands the upper half
//! to the idle connection. The original connection notices its shrunken `end`
//! on its next chunk and stops there. This keeps all connections busy right to
//! the end of the file, which is where naïve static splitting loses most time.

use crate::model::SegmentView;

/// End offset used for streams of unknown length.
pub(crate) const UNBOUNDED: u64 = u64::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    pub start: u64,
    /// Exclusive. [`UNBOUNDED`] when the total size is unknown.
    pub end: u64,
    /// Bytes received from the network (may still be buffered in memory).
    pub received: u64,
    /// Bytes durably handed to the OS. Only this is persisted for resume.
    pub written: u64,
    pub active: bool,
}

impl Segment {
    pub fn new(start: u64, end: u64, written: u64) -> Self {
        Self { start, end, received: written, written, active: false }
    }

    /// Absolute offset of the next byte to receive.
    pub fn cursor(&self) -> u64 {
        self.start + self.received
    }

    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.cursor())
    }

    pub fn is_done(&self) -> bool {
        self.start + self.written >= self.end
    }
}

#[derive(Debug, Default)]
pub(crate) struct SegmentTable {
    pub segments: Vec<Segment>,
}

impl SegmentTable {
    /// Splits `[0, total)` into up to `parts` equal segments, each at least `min_size` long.
    pub fn split_evenly(total: u64, parts: usize, min_size: u64) -> Self {
        let parts = (total / min_size.max(1)).clamp(1, parts.max(1) as u64);
        let step = total / parts;
        let segments = (0..parts)
            .map(|i| {
                let start = i * step;
                let end = if i + 1 == parts { total } else { start + step };
                Segment::new(start, end, 0)
            })
            .collect();
        Self { segments }
    }

    pub fn single(end: u64) -> Self {
        Self { segments: vec![Segment::new(0, end, 0)] }
    }

    /// Hands out work: first an idle unfinished segment, otherwise half of the
    /// busiest one. Returns the segment index, now marked active.
    pub fn claim(&mut self, min_split: u64) -> Option<usize> {
        if let Some(i) = self.segments.iter().position(|s| !s.active && !s.is_done()) {
            let s = &mut self.segments[i];
            s.received = s.written;
            s.active = true;
            return Some(i);
        }
        let (i, remaining) = self
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.active && s.end != UNBOUNDED)
            .map(|(i, s)| (i, s.remaining()))
            .max_by_key(|&(_, r)| r)?;
        if remaining < min_split.saturating_mul(2) {
            return None;
        }
        let victim = &mut self.segments[i];
        let mid = victim.cursor() + remaining / 2;
        let end = victim.end;
        victim.end = mid;
        let mut stolen = Segment::new(mid, end, 0);
        stolen.active = true;
        self.segments.push(stolen);
        Some(self.segments.len() - 1)
    }

    /// Returns a segment to the pool, discarding any unwritten bytes.
    pub fn release(&mut self, i: usize) {
        let s = &mut self.segments[i];
        s.active = false;
        s.received = s.written;
    }

    pub fn is_complete(&self) -> bool {
        self.segments.iter().all(Segment::is_done)
    }

    /// Bytes received so far (including in-memory buffers).
    pub fn received(&self) -> u64 {
        self.segments.iter().map(|s| s.received).sum()
    }

    pub fn written(&self) -> u64 {
        self.segments.iter().map(|s| s.written).sum()
    }

    pub fn active(&self) -> usize {
        self.segments.iter().filter(|s| s.active).count()
    }

    pub fn views(&self) -> Vec<SegmentView> {
        let mut v: Vec<_> = self
            .segments
            .iter()
            .map(|s| SegmentView { start: s.start, end: s.end, done: s.received, active: s.active })
            .collect();
        v.sort_by_key(|s| s.start);
        v
    }

    /// Persistable form: `(start, end, written)`.
    pub fn to_persisted(&self) -> Vec<(u64, u64, u64)> {
        let mut v: Vec<_> = self.segments.iter().map(|s| (s.start, s.end, s.written)).collect();
        v.sort_unstable();
        v
    }

    /// Rebuilds a table from persisted data, rejecting anything inconsistent
    /// with `total` (overlaps, gaps, out-of-range values).
    pub fn from_persisted(parts: &[(u64, u64, u64)], total: u64) -> Option<Self> {
        let mut parts = parts.to_vec();
        parts.sort_unstable();
        let mut expected_start = 0;
        for &(start, end, written) in &parts {
            if start != expected_start || end <= start || written > end - start {
                return None;
            }
            expected_start = end;
        }
        if expected_start != total {
            return None;
        }
        Some(Self { segments: parts.into_iter().map(|(s, e, w)| Segment::new(s, e, w)).collect() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    #[test]
    fn even_split_respects_minimum() {
        let t = SegmentTable::split_evenly(10 * MIB, 8, MIB);
        assert_eq!(t.segments.len(), 8);
        assert_eq!(t.segments.last().unwrap().end, 10 * MIB);
        let t = SegmentTable::split_evenly(3 * MIB, 8, MIB);
        assert_eq!(t.segments.len(), 3);
        let t = SegmentTable::split_evenly(100, 8, MIB);
        assert_eq!(t.segments.len(), 1);
    }

    #[test]
    fn claim_prefers_idle_then_steals() {
        let mut t = SegmentTable::split_evenly(8 * MIB, 2, MIB);
        assert_eq!(t.claim(MIB), Some(0));
        assert_eq!(t.claim(MIB), Some(1));
        // both busy: steal from the larger remaining
        t.segments[0].received = 3 * MIB; // 1 MiB left
        let i = t.claim(MIB).unwrap();
        assert_eq!(i, 2);
        assert_eq!(t.segments[1].end, 6 * MIB);
        assert_eq!(t.segments[2].start, 6 * MIB);
        assert_eq!(t.segments[2].end, 8 * MIB);
        // tiny remainders are not split
        t.segments[1].received = 2 * MIB - 1;
        t.segments[2].received = 2 * MIB - 1;
        assert_eq!(t.claim(MIB), None);
    }

    #[test]
    fn persistence_roundtrip_and_validation() {
        let mut t = SegmentTable::split_evenly(10 * MIB, 4, MIB);
        t.segments[1].written = 1234;
        let p = t.to_persisted();
        let back = SegmentTable::from_persisted(&p, 10 * MIB).unwrap();
        assert_eq!(back.written(), 1234);
        assert!(SegmentTable::from_persisted(&p, 11 * MIB).is_none());
        let mut gap = p.clone();
        gap.remove(1);
        assert!(SegmentTable::from_persisted(&gap, 10 * MIB).is_none());
    }
}
