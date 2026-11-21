//! MAtricks: turning a fixture selection into ordered cells.
//!
//! MAtricks is a selection tool, not a value store. Blocks merge consecutive
//! fixtures into one cell, groups deal cells into interleaved lanes, wings fold
//! the lanes from their outside ends, and width transposes wrapped rows into
//! columns. A cell is the unit `Align` writes: every fixture in one cell gets
//! the same value.
//!
//! The implementation is deliberately one-dimensional. It has no fixture XYZ
//! positions, so it cannot build a real 3D selection grid; it works on the
//! selection order the operator addressed. Shuffle is deterministic for a
//! given seed, and every transform is a pure function, which is what makes the
//! command-line behaviour testable without a display.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Largest block, group, wing or width accepted.
pub const MAX_MATRICKS: u16 = 4096;
/// Largest shuffle seed accepted. Zero means shuffle off.
pub const MAX_SHUFFLE_SEED: u16 = 32767;

fn one() -> usize {
    1
}

fn none() -> usize {
    0
}

/// The MAtricks settings for the current selection.
///
/// This is desk state: it belongs to the operator's current working selection,
/// not to the show. A new selection keeps the settings so a following `Shuffle`
/// or `Align` applies the same shape; `MAtricks Reset` starts again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Matricks {
    /// Consecutive fixtures merged into one cell. One means no blocking.
    pub block: usize,
    /// Interleaved lanes. One means the selection stays in addressed order.
    pub group: usize,
    /// Lanes folded from their outside ends. One means no wings.
    pub wings: usize,
    /// Cells per wrapped row before the rest move to the next row. Zero or one
    /// means no wrapping.
    pub width: usize,
    /// Non-zero is the deterministic shuffle seed.
    pub shuffle: u16,
    /// Mirror the aligned values around the centre cell.
    pub mirror: bool,
}

impl Default for Matricks {
    fn default() -> Self {
        Self {
            block: one(),
            group: one(),
            wings: one(),
            width: none(),
            shuffle: 0,
            mirror: false,
        }
    }
}

/// One settable MAtricks property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatricksProperty {
    Block,
    Group,
    Wings,
    Width,
    Shuffle,
}

impl MatricksProperty {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "block" | "xblock" => Some(Self::Block),
            "group" | "xgroup" => Some(Self::Group),
            "wings" | "xwings" => Some(Self::Wings),
            "width" | "xwidth" => Some(Self::Width),
            "shuffle" | "xshuffle" => Some(Self::Shuffle),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Block => "XBlock",
            Self::Group => "XGroup",
            Self::Wings => "XWings",
            Self::Width => "XWidth",
            Self::Shuffle => "XShuffle",
        }
    }
}

impl Matricks {
    /// Set one property. Block, group and wings treat zero as "off", which is
    /// the source's `None`; width treats one as no wrapping.
    pub fn set(&mut self, property: MatricksProperty, value: u16) {
        let value = usize::from(value);
        match property {
            MatricksProperty::Block => self.block = value.max(1),
            MatricksProperty::Group => self.group = value.max(1),
            MatricksProperty::Wings => self.wings = value.max(1),
            MatricksProperty::Width => self.width = value,
            MatricksProperty::Shuffle => {
                self.shuffle = value.min(usize::from(MAX_SHUFFLE_SEED)) as u16;
            }
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Drop values a hand-edited desk file cannot mean, reporting each repair.
    pub fn sanitise(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        for (label, value) in [
            ("block", &mut self.block),
            ("group", &mut self.group),
            ("wings", &mut self.wings),
        ] {
            let original = *value;
            if original == 0 {
                *value = 1;
                warnings.push(format!(
                    "desk: MAtricks {label} 0 is not a size; reset to 1"
                ));
            } else if original > usize::from(MAX_MATRICKS) {
                *value = usize::from(MAX_MATRICKS);
                warnings.push(format!(
                    "desk: MAtricks {label} {original} is above {MAX_MATRICKS}; clamped"
                ));
            }
        }
        if self.width > usize::from(MAX_MATRICKS) {
            let original = self.width;
            self.width = usize::from(MAX_MATRICKS);
            warnings.push(format!(
                "desk: MAtricks width {original} is above {MAX_MATRICKS}; clamped"
            ));
        }
        if self.shuffle > MAX_SHUFFLE_SEED {
            let original = self.shuffle;
            self.shuffle = MAX_SHUFFLE_SEED;
            warnings.push(format!(
                "desk: MAtricks shuffle {original} is above {MAX_SHUFFLE_SEED}; clamped"
            ));
        }
        warnings
    }

    /// Build the ordered cells for a selection.
    ///
    /// The pipeline is block, group, wings, width. Each step works on the cells
    /// produced by the previous one, so the settings compose in a predictable
    /// order rather than being four unrelated switches.
    pub fn cells(&self, selection: &[u16]) -> Vec<Vec<u16>> {
        let mut seen = BTreeSet::new();
        let fixtures: Vec<u16> = selection
            .iter()
            .copied()
            .filter(|number| seen.insert(*number))
            .collect();
        if fixtures.is_empty() {
            return Vec::new();
        }

        let fixtures = if self.shuffle == 0 {
            fixtures
        } else {
            shuffled(&fixtures, self.shuffle)
        };

        let mut cells: Vec<Vec<u16>> = fixtures
            .chunks(self.block.max(1))
            .map(|chunk| chunk.to_vec())
            .collect();

        let groups = self.group.clamp(1, cells.len());
        if groups > 1 {
            let mut lanes: Vec<Vec<u16>> = vec![Vec::new(); groups];
            for (index, cell) in cells.into_iter().enumerate() {
                lanes[index % groups].extend(cell);
            }
            cells = lanes.into_iter().filter(|lane| !lane.is_empty()).collect();
        }

        let wings = self.wings.clamp(1, cells.len());
        if wings > 1 {
            let count = cells.len();
            let mut lanes: Vec<Vec<Vec<u16>>> = Vec::with_capacity(wings);
            for wing in 0..wings {
                let start = wing * count / wings;
                let end = (wing + 1) * count / wings;
                lanes.push(cells[start..end].to_vec());
            }
            let depth = lanes.iter().map(Vec::len).max().unwrap_or(0);
            let mut folded = Vec::with_capacity(depth);
            for level in 0..depth {
                let mut cell = Vec::new();
                for (wing, lane) in lanes.iter().enumerate() {
                    if level >= lane.len() {
                        continue;
                    }
                    // Every other wing reads from its far end, so the two
                    // outside fixtures share the first cell.
                    let index = if wing % 2 == 0 {
                        level
                    } else {
                        lane.len() - 1 - level
                    };
                    cell.extend(lane[index].iter().copied());
                }
                folded.push(cell);
            }
            cells = folded;
        }

        if self.width >= 2 && cells.len() > self.width {
            let width = self.width;
            let rows: Vec<&[Vec<u16>]> = cells.chunks(width).collect();
            let mut columns = Vec::with_capacity(width);
            for column in 0..width {
                let mut cell = Vec::new();
                for row in &rows {
                    if let Some(part) = row.get(column) {
                        cell.extend(part.iter().copied());
                    }
                }
                if !cell.is_empty() {
                    columns.push(cell);
                }
            }
            cells = columns;
        }

        cells
    }
}

/// An alignment curve. The names follow the manual's `AlignTransition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlignTransition {
    #[default]
    Linear,
    Sinus,
    Slow,
    Fast,
}

impl AlignTransition {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "linear" | "0" => Some(Self::Linear),
            "sinus" | "1" => Some(Self::Sinus),
            "slow" | "2" => Some(Self::Slow),
            "fast" | "3" => Some(Self::Fast),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Sinus => "Sinus",
            Self::Slow => "Slow",
            Self::Fast => "Fast",
        }
    }

    pub fn names() -> &'static str {
        "Linear, Sinus, Slow or Fast"
    }

    /// Shape a 0..=1 position. Sinus has small gaps at both ends and large
    /// gaps in the middle; Slow grows from small gaps; Fast starts with large
    /// gaps and settles down.
    pub fn apply(self, position: f64) -> f64 {
        let position = position.clamp(0.0, 1.0);
        match self {
            Self::Linear => position,
            Self::Sinus => 0.5 - 0.5 * (std::f64::consts::PI * position).cos(),
            Self::Slow => position * position,
            Self::Fast => 1.0 - (1.0 - position) * (1.0 - position),
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Linear => Self::Sinus,
            Self::Sinus => Self::Slow,
            Self::Slow => Self::Fast,
            Self::Fast => Self::Linear,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Linear => Self::Fast,
            Self::Sinus => Self::Linear,
            Self::Slow => Self::Sinus,
            Self::Fast => Self::Slow,
        }
    }
}

/// Values for `cell_count` cells from `from` to `to`.
///
/// A single cell takes `to`: it is both the first and the last cell, so the
/// end of the range is the only value that can be written without inventing a
/// second step. With mirror, the first ceil(count / 2) cells carry the range
/// and the rest are copied back from the centre, leaving the centre cell (or
/// centre pair) at `to`.
pub fn aligned_levels(
    cell_count: usize,
    from: f64,
    to: f64,
    transition: AlignTransition,
    mirror: bool,
) -> Vec<f64> {
    if cell_count == 0 {
        return Vec::new();
    }
    if cell_count == 1 {
        return vec![to];
    }
    let span = if mirror {
        cell_count.div_ceil(2)
    } else {
        cell_count
    };
    let last = (span - 1).max(1) as f64;
    let mut values: Vec<f64> = (0..span)
        .map(|index| {
            let position = if span == 1 { 1.0 } else { index as f64 / last };
            from + (to - from) * transition.apply(position)
        })
        .collect();
    if mirror {
        for index in span..cell_count {
            values.push(values[cell_count - 1 - index]);
        }
    }
    values
}

/// Fisher-Yates over a tiny SplitMix64 stream. The same seed and fixture order
/// always produce the same permutation; no clock or OS random source is used.
fn shuffled(fixtures: &[u16], seed: u16) -> Vec<u16> {
    let mut result = fixtures.to_vec();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64 ^ u64::from(seed);
    for index in (1..result.len()).rev() {
        let pick = (next_u64(&mut state) % (index as u64 + 1)) as usize;
        result.swap(index, pick);
    }
    result
}

fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(matricks: Matricks, selection: &[u16]) -> Vec<Vec<u16>> {
        matricks.cells(selection)
    }

    #[test]
    fn defaults_leave_every_fixture_alone() {
        let matricks = Matricks::default();
        assert_eq!(cells(matricks, &[1, 2, 3]), vec![vec![1], vec![2], vec![3]]);
        assert!(matricks.is_default());
    }

    #[test]
    fn duplicate_numbers_keep_their_first_position() {
        assert_eq!(
            cells(Matricks::default(), &[3, 1, 3, 2, 1]),
            vec![vec![3], vec![1], vec![2]]
        );
    }

    #[test]
    fn blocks_merge_consecutive_fixtures() {
        let matricks = Matricks {
            block: 2,
            ..Matricks::default()
        };
        assert_eq!(
            cells(matricks, &[1, 2, 3, 4, 5]),
            vec![vec![1, 2], vec![3, 4], vec![5]]
        );
    }

    #[test]
    fn groups_deal_cells_into_interleaved_lanes() {
        let matricks = Matricks {
            group: 2,
            ..Matricks::default()
        };
        assert_eq!(
            cells(matricks, &[1, 2, 3, 4, 5]),
            vec![vec![1, 3, 5], vec![2, 4]]
        );
    }

    #[test]
    fn wings_fold_from_the_outside_ends() {
        let matricks = Matricks {
            wings: 2,
            ..Matricks::default()
        };
        assert_eq!(
            cells(matricks, &[1, 2, 3, 4, 5, 6]),
            vec![vec![1, 6], vec![2, 5], vec![3, 4]]
        );
    }

    #[test]
    fn width_transposes_wrapped_rows_into_columns() {
        let matricks = Matricks {
            width: 3,
            ..Matricks::default()
        };
        assert_eq!(
            cells(matricks, &[1, 2, 3, 4, 5, 6, 7]),
            vec![vec![1, 4, 7], vec![2, 5], vec![3, 6]]
        );
    }

    #[test]
    fn blocks_groups_wings_and_width_compose_in_order() {
        let matricks = Matricks {
            block: 2,
            group: 2,
            wings: 2,
            width: 0,
            shuffle: 0,
            mirror: false,
        };
        // Blocks: [1 2] [3 4] [5 6]. Groups deal them to
        // [1 2] [5 6] | [3 4]. Wings then fold the two group cells together,
        // and there is no width left to transpose.
        assert_eq!(
            cells(matricks, &[1, 2, 3, 4, 5, 6]),
            vec![vec![1, 2, 5, 6, 3, 4]]
        );
    }

    #[test]
    fn shuffle_is_deterministic_and_keeps_the_selection() {
        let selection: Vec<u16> = (1..=10).collect();
        let matricks = Matricks {
            shuffle: 42,
            ..Matricks::default()
        };
        let first = cells(matricks, &selection);
        let second = cells(matricks, &selection);
        assert_eq!(first, second);
        let mut flat: Vec<u16> = first.into_iter().flatten().collect();
        flat.sort_unstable();
        assert_eq!(flat, selection);

        let different = cells(
            Matricks {
                shuffle: 43,
                ..Matricks::default()
            },
            &selection,
        );
        assert_ne!(cells(matricks, &selection), different);
    }

    #[test]
    fn a_zero_seed_never_shuffles() {
        let matricks = Matricks {
            shuffle: 0,
            ..Matricks::default()
        };
        assert_eq!(cells(matricks, &[1, 2, 3]), vec![vec![1], vec![2], vec![3]]);
    }

    #[test]
    fn align_is_linear_by_default() {
        let values = aligned_levels(3, 0.0, 1.0, AlignTransition::Linear, false);
        assert_eq!(values, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn a_single_cell_takes_the_end_value() {
        assert_eq!(
            aligned_levels(1, 25.0, 75.0, AlignTransition::Linear, false),
            vec![75.0]
        );
    }

    #[test]
    fn mirror_centres_the_end_value() {
        let values = aligned_levels(6, 0.0, 1.0, AlignTransition::Linear, true);
        assert_eq!(values, vec![0.0, 0.5, 1.0, 1.0, 0.5, 0.0]);
        let odd = aligned_levels(5, 0.0, 1.0, AlignTransition::Linear, true);
        assert_eq!(odd, vec![0.0, 0.5, 1.0, 0.5, 0.0]);
    }

    #[test]
    fn transitions_shape_the_gaps() {
        let slow = aligned_levels(3, 0.0, 1.0, AlignTransition::Slow, false);
        assert_eq!(slow, vec![0.0, 0.25, 1.0]);
        let fast = aligned_levels(3, 0.0, 1.0, AlignTransition::Fast, false);
        assert_eq!(fast, vec![0.0, 0.75, 1.0]);
        let sinus = aligned_levels(3, 0.0, 1.0, AlignTransition::Sinus, false);
        assert!((sinus[0] - 0.0).abs() < 1e-12);
        assert!((sinus[1] - 0.5).abs() < 1e-12);
        assert!((sinus[2] - 1.0).abs() < 1e-12);
        let quarter = AlignTransition::Sinus.apply(0.25);
        assert!((quarter - 0.146_446_609_406_726).abs() < 1e-12);
    }

    #[test]
    fn transition_names_round_trip() {
        for transition in [
            AlignTransition::Linear,
            AlignTransition::Sinus,
            AlignTransition::Slow,
            AlignTransition::Fast,
        ] {
            assert_eq!(AlignTransition::parse(transition.label()), Some(transition));
            assert_eq!(transition.next().previous(), transition);
        }
        assert_eq!(AlignTransition::parse("2"), Some(AlignTransition::Slow));
        assert_eq!(AlignTransition::parse("none"), None);
    }

    #[test]
    fn sanitise_repairs_impossible_settings() {
        let mut matricks = Matricks {
            block: 0,
            group: usize::from(MAX_MATRICKS) + 1,
            wings: 1,
            width: usize::from(MAX_MATRICKS) + 1,
            shuffle: MAX_SHUFFLE_SEED + 1,
            mirror: false,
        };
        let warnings = matricks.sanitise();
        assert_eq!(matricks.block, 1);
        assert_eq!(matricks.group, usize::from(MAX_MATRICKS));
        assert_eq!(matricks.width, usize::from(MAX_MATRICKS));
        assert_eq!(matricks.shuffle, MAX_SHUFFLE_SEED);
        assert_eq!(warnings.len(), 4);
    }
}
