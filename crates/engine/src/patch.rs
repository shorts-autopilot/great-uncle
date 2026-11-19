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
    fn fits_respects_universe_size() {
        assert!(PatchRange::new(1, 509, 4).unwrap().fits(512));
        assert!(!PatchRange::new(1, 510, 4).unwrap().fits(512));
    }
}
