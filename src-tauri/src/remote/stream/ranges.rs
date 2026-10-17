// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Which bytes of a sparse cache file hold real data.

use std::io;
use std::path::Path;

/// Half-open `[start, end)` spans, kept sorted and merged so adjacent or
/// overlapping writes collapse into one span.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RangeSet {
    spans: Vec<(u64, u64)>,
}

impl RangeSet {
    pub fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        // Spans that overlap or touch [start, end) fold into it.
        let first = self.spans.partition_point(|&(_, e)| e < start);
        let last = self.spans.partition_point(|&(s, _)| s <= end);
        let mut merged = (start, end);
        if first < last {
            merged.0 = merged.0.min(self.spans[first].0);
            merged.1 = merged.1.max(self.spans[last - 1].1);
        }
        self.spans.splice(first..last, [merged]);
    }

    /// Bytes available starting at `pos`, 0 when `pos` is not covered.
    pub fn contiguous_from(&self, pos: u64) -> u64 {
        let i = self.spans.partition_point(|&(_, e)| e <= pos);
        match self.spans.get(i) {
            Some(&(s, e)) if s <= pos => e - pos,
            _ => 0,
        }
    }

    /// The first uncovered byte in `[from, len)`.
    pub fn first_gap(&self, from: u64, len: u64) -> Option<u64> {
        let gap = from + self.contiguous_from(from);
        (gap < len).then_some(gap)
    }

    /// Where the next span after `pos` begins, for capping a write so it
    /// stops at data that is already there.
    pub fn next_start_after(&self, pos: u64) -> Option<u64> {
        let i = self.spans.partition_point(|&(s, _)| s <= pos);
        self.spans.get(i).map(|&(s, _)| s)
    }

    pub fn covers(&self, len: u64) -> bool {
        self.first_gap(0, len).is_none()
    }

    /// One past the last covered byte.
    pub fn end(&self) -> u64 {
        self.spans.last().map_or(0, |&(_, e)| e)
    }

    /// `None` when the sidecar is missing or any line is unreadable.
    pub fn load(sidecar: &Path) -> Option<RangeSet> {
        let text = std::fs::read_to_string(sidecar).ok()?;
        let mut ranges = RangeSet::default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let (start, end) = line.trim().split_once('-')?;
            ranges.insert(start.parse().ok()?, end.parse().ok()?);
        }
        Some(ranges)
    }

    /// Written beside the sidecar and renamed over it, so a crash leaves the
    /// old list or the new one, never half of either.
    pub fn save(&self, sidecar: &Path) -> io::Result<()> {
        let mut text = String::new();
        for (start, end) in &self.spans {
            text.push_str(&format!("{start}-{end}\n"));
        }
        let mut temp = sidecar.as_os_str().to_owned();
        temp.push(".tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, sidecar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn set(spans: &[(u64, u64)]) -> RangeSet {
        let mut ranges = RangeSet::default();
        for &(start, end) in spans {
            ranges.insert(start, end);
        }
        ranges
    }

    #[test]
    fn merges_overlapping_and_adjacent_spans() {
        let ranges = set(&[(10, 20), (30, 40), (20, 25), (35, 50), (0, 2)]);
        assert_eq!(ranges.spans, vec![(0, 2), (10, 25), (30, 50)]);

        let ranges = set(&[(10, 20), (30, 40), (5, 45)]);
        assert_eq!(ranges.spans, vec![(5, 45)]);

        let ranges = set(&[(10, 20), (20, 30), (30, 40), (7, 7)]);
        assert_eq!(ranges.spans, vec![(10, 40)]);
    }

    #[test]
    fn contiguous_from_and_first_gap() {
        let ranges = set(&[(0, 100), (200, 300)]);
        assert_eq!(ranges.contiguous_from(0), 100);
        assert_eq!(ranges.contiguous_from(50), 50);
        assert_eq!(ranges.contiguous_from(100), 0);
        assert_eq!(ranges.contiguous_from(250), 50);
        assert_eq!(ranges.contiguous_from(300), 0);

        assert_eq!(ranges.first_gap(0, 400), Some(100));
        assert_eq!(ranges.first_gap(150, 400), Some(150));
        assert_eq!(ranges.first_gap(200, 400), Some(300));
        assert_eq!(ranges.first_gap(200, 300), None);
        assert_eq!(ranges.first_gap(400, 400), None);
        assert_eq!(RangeSet::default().first_gap(0, 10), Some(0));

        assert_eq!(ranges.next_start_after(100), Some(200));
        assert_eq!(ranges.next_start_after(250), None);

        assert!(!ranges.covers(300));
        assert!(set(&[(0, 300)]).covers(300));
        assert!(RangeSet::default().covers(0));
        assert_eq!(ranges.end(), 300);
    }

    #[test]
    fn sidecar_round_trip() {
        let dir = std::env::temp_dir().join(format!("wave-test-ranges-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let sidecar = dir.join("1.flac.ranges");

        assert_eq!(RangeSet::load(&sidecar), None);
        let ranges = set(&[(0, 1024), (4096, 8192)]);
        ranges.save(&sidecar).unwrap();
        assert_eq!(
            std::fs::read_to_string(&sidecar).unwrap(),
            "0-1024\n4096-8192\n"
        );
        assert_eq!(RangeSet::load(&sidecar), Some(ranges));
        assert!(!dir.join("1.flac.ranges.tmp").exists());

        std::fs::write(&sidecar, "0-10\nnonsense\n").unwrap();
        assert_eq!(RangeSet::load(&sidecar), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
