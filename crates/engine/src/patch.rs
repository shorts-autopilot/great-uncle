//! Patch arithmetic: address ranges and overlaps.
//!
//! A patch entry occupies `footprint` consecutive slots starting at `address`
//! inside one universe. Overlaps are rejected when patching and when loading a
//! show, so a fixture never drives another fixture's slots.

/// Inclusive slot range of one patched fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchRange {
    pub universe: u16,
    pub start: u16,
    pub end: u16,
}

impl PatchRange {
    /// `None` when the address or footprint is zero, or the range overflows.
    pub fn new(universe: u16, address: u16, footprint: u16) -> Option<Self> {
        if address == 0 || footprint == 0 {
            return None;
        }
        let end = address.checked_add(footprint - 1)?;
        Some(Self {
            universe,
            start: address,
            end,
        })
    }

    pub fn footprint(&self) -> u16 {
        self.end - self.start + 1
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.universe == other.universe && self.start <= other.end && other.start <= self.end
    }

    /// Does this range sit completely inside a universe of `size` slots?
    pub fn fits(&self, size: u16) -> bool {
        self.end <= size
    }
}

/// Unpatched runs in one universe, as `(start, length)`, in ascending start
/// order. Runs of exactly `minimum` slots are not reported: the patch check
/// asks for gaps larger than a threshold.
///
/// Ranges are clipped to `size` and overlaps are merged, so a broken patch still
/// produces sensible gaps instead of double-counting or underflowing.
pub fn gaps(ranges: &[PatchRange], size: u16, minimum: u16) -> Vec<(u16, u16)> {
    let mut spans: Vec<(u16, u16)> = ranges
        .iter()
        .filter(|range| range.start <= size)
        .map(|range| (range.start, range.end.min(size)))
        .collect();
    spans.sort_unstable();

    let mut runs = Vec::new();
    let mut cursor = 1u16;
    for (start, end) in spans {
        if start > cursor {
            let length = start - cursor;
            if length > minimum {
                runs.push((cursor, length));
            }
        }
        cursor = cursor.max(end.saturating_add(1));
        if cursor > size {
            break;
        }
    }
    if cursor <= size {
        let length = size - cursor + 1;
        if length > minimum {
            runs.push((cursor, length));
        }
    }
    runs
}

/// All overlapping pairs, as `(lower fixture number, higher fixture number)`.
pub fn conflicts(ranges: &[(u16, PatchRange)]) -> Vec<(u16, u16)> {
    let mut pairs = Vec::new();
    for (index, (number, range)) in ranges.iter().enumerate() {
        for (other_number, other) in &ranges[index + 1..] {
            if range.overlaps(other) {
                pairs.push(((*number).min(*other_number), (*number).max(*other_number)));
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_inclusive() {
        let range = PatchRange::new(1, 10, 4).unwrap();
        assert_eq!((range.start, range.end, range.footprint()), (10, 13, 4));
        assert!(PatchRange::new(1, 0, 4).is_none());
        assert!(PatchRange::new(1, 10, 0).is_none());
        assert!(PatchRange::new(1, u16::MAX, 2).is_none(), "overflow");
    }

    #[test]
    fn adjacency_is_not_overlap() {
        let first = PatchRange::new(1, 1, 4).unwrap();
        let second = PatchRange::new(1, 5, 4).unwrap();
        assert!(!first.overlaps(&second));
        let third = PatchRange::new(1, 4, 4).unwrap();
        assert!(first.overlaps(&third));
    }

    #[test]
    fn different_universes_never_conflict() {
        let a = PatchRange::new(1, 1, 10).unwrap();
        let b = PatchRange::new(2, 1, 10).unwrap();
        assert!(!a.overlaps(&b));
        assert!(conflicts(&[(1, a), (2, b)]).is_empty());
    }

    #[test]
    fn conflicts_are_reported_once_per_pair() {
        let ranges = vec![
            (3, PatchRange::new(1, 1, 4).unwrap()),
            (7, PatchRange::new(1, 3, 4).unwrap()),
            (9, PatchRange::new(1, 100, 1).unwrap()),
        ];
        assert_eq!(conflicts(&ranges), vec![(3, 7)]);
    }

    #[test]
    fn gaps_report_runs_longer_than_the_minimum() {
        let ranges = [
            PatchRange::new(1, 1, 4).unwrap(),
            PatchRange::new(1, 10, 3).unwrap(),
        ];
        assert_eq!(gaps(&ranges, 20, 2), vec![(5, 5), (13, 8)]);
        assert_eq!(
            gaps(&ranges, 20, 8),
            Vec::new(),
            "a run of exactly the minimum is not reported"
        );
    }

    #[test]
    fn a_range_ending_at_the_universe_size_has_no_trailing_gap() {
        let ranges = [
            PatchRange::new(1, 1, 4).unwrap(),
            PatchRange::new(1, 8, 13).unwrap(),
        ];
        assert_eq!(gaps(&ranges, 20, 2), vec![(5, 3)]);
    }

    #[test]
    fn a_full_universe_has_no_gaps() {
        assert!(gaps(&[PatchRange::new(1, 1, 20).unwrap()], 20, 0).is_empty());
        assert_eq!(
            gaps(&[], 20, 2),
            vec![(1, 20)],
            "an empty universe is one big gap"
        );
    }

    #[test]
    fn overlapping_ranges_are_merged_before_gaps_are_measured() {
        let ranges = [
            PatchRange::new(1, 1, 10).unwrap(),
            PatchRange::new(1, 5, 11).unwrap(),
        ];
        assert_eq!(gaps(&ranges, 20, 2), vec![(16, 5)]);
    }

    #[test]
    fn fits_respects_universe_size() {
        assert!(PatchRange::new(1, 509, 4).unwrap().fits(512));
        assert!(!PatchRange::new(1, 510, 4).unwrap().fits(512));
    }
}
