//! The desktop's user-defined area.
//!
//! A screen is a 12 × 8 snap grid. Windows are placed, moved and resized by
//! grid cells and may never overlap. This module is headless: it knows nothing
//! about egui, the engine or show state, so placement can be tested without a
//! display.

/// Grid columns in the user-defined area.
pub const COLUMNS: u16 = 12;
/// Grid rows in the user-defined area.
pub const ROWS: u16 = 8;

/// Stable identity for one window on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// A rectangle in grid cells. The origin is the upper-left cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl GridRect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    fn overlaps(self, other: GridRect) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }
}

/// The window types that can be filled from the current snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WindowKind {
    FixtureSheet,
    SequenceSheet,
    Playbacks,
    Pools,
    Presets,
    Groups,
    Worlds,
    Filters,
    Effects,
    Timecode,
    CommandLineHistory,
    DmxSheet,
    Masters,
    RunningPlaybacks,
    Patch,
    Status,
}

impl WindowKind {
    /// Alphabetical, like the manual's All tab.
    pub const ALL: [WindowKind; 16] = [
        WindowKind::CommandLineHistory,
        WindowKind::DmxSheet,
        WindowKind::Effects,
        WindowKind::Filters,
        WindowKind::FixtureSheet,
        WindowKind::Groups,
        WindowKind::Masters,
        WindowKind::Patch,
        WindowKind::Playbacks,
        WindowKind::Pools,
        WindowKind::Presets,
        WindowKind::RunningPlaybacks,
        WindowKind::SequenceSheet,
        WindowKind::Status,
        WindowKind::Timecode,
        WindowKind::Worlds,
    ];

    pub fn title(self) -> &'static str {
        match self {
            WindowKind::FixtureSheet => "Fixture Sheet",
            WindowKind::SequenceSheet => "Sequence Sheet",
            WindowKind::Playbacks => "Playbacks",
            WindowKind::Pools => "Pools",
            WindowKind::Presets => "Presets",
            WindowKind::Groups => "Groups",
            WindowKind::Worlds => "Worlds",
            WindowKind::Filters => "Filters",
            WindowKind::Effects => "Effects",
            WindowKind::Timecode => "Timecode Viewer",
            WindowKind::CommandLineHistory => "Command Line History",
            WindowKind::DmxSheet => "DMX Sheet",
            WindowKind::Masters => "Masters",
            WindowKind::RunningPlaybacks => "Running Playbacks",
            WindowKind::Patch => "Patch",
            WindowKind::Status => "Status",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            WindowKind::FixtureSheet => "Patched fixtures, levels and colour",
            WindowKind::SequenceSheet => "Cues, triggers, timings and values",
            WindowKind::Playbacks => "Nine executor faders and the grand master",
            WindowKind::Pools => "Palettes, groups, worlds, filters and effects",
            WindowKind::Presets => "The palette pool",
            WindowKind::Groups => "Stored fixture groups",
            WindowKind::Worlds => "Stored worlds",
            WindowKind::Filters => "Stored attribute filters",
            WindowKind::Effects => "Stored effects and their run state",
            WindowKind::Timecode => "Timecode shows, tracks and markers",
            WindowKind::CommandLineHistory => "Commands and their results",
            WindowKind::DmxSheet => "Rendered DMX slot values",
            WindowKind::Masters => "Grand, rate, tempo and page",
            WindowKind::RunningPlaybacks => "Playbacks that are on stage",
            WindowKind::Patch => "Fixture numbers, types, modes and addresses",
            WindowKind::Status => "The same lines as the Status command",
        }
    }

    /// Default placement size, in grid cells.
    pub fn default_size(self) -> (u16, u16) {
        match self {
            WindowKind::FixtureSheet => (8, 5),
            WindowKind::SequenceSheet => (8, 5),
            WindowKind::Playbacks => (5, 4),
            WindowKind::Pools => (5, 4),
            WindowKind::Presets => (4, 3),
            WindowKind::Groups => (4, 3),
            WindowKind::Worlds => (4, 3),
            WindowKind::Filters => (4, 3),
            WindowKind::Effects => (4, 3),
            WindowKind::Timecode => (7, 4),
            WindowKind::CommandLineHistory => (6, 4),
            WindowKind::DmxSheet => (6, 4),
            WindowKind::Masters => (4, 3),
            WindowKind::RunningPlaybacks => (5, 3),
            WindowKind::Patch => (6, 3),
            WindowKind::Status => (5, 4),
        }
    }

    /// The smallest useful size, in grid cells.
    pub fn min_size(self) -> (u16, u16) {
        match self {
            WindowKind::FixtureSheet | WindowKind::SequenceSheet => (4, 3),
            WindowKind::Pools | WindowKind::Timecode | WindowKind::DmxSheet => (3, 3),
            WindowKind::CommandLineHistory | WindowKind::Patch | WindowKind::RunningPlaybacks => {
                (3, 2)
            }
            WindowKind::Playbacks | WindowKind::Presets | WindowKind::Groups => (3, 2),
            WindowKind::Worlds | WindowKind::Filters | WindowKind::Effects => (3, 2),
            WindowKind::Masters | WindowKind::Status => (3, 2),
        }
    }
}

/// Add Window picker tabs. `All` is the union; the others mirror the
/// grandMA3 categories this build can populate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowTab {
    Common,
    Pools,
    Playback,
    Tools,
    All,
}

impl WindowTab {
    pub const ALL: [WindowTab; 5] = [
        WindowTab::Common,
        WindowTab::Pools,
        WindowTab::Playback,
        WindowTab::Tools,
        WindowTab::All,
    ];

    pub fn title(self) -> &'static str {
        match self {
            WindowTab::Common => "Common",
            WindowTab::Pools => "Pools",
            WindowTab::Playback => "Playback",
            WindowTab::Tools => "Tools",
            WindowTab::All => "All",
        }
    }

    pub fn kinds(self) -> &'static [WindowKind] {
        const COMMON: &[WindowKind] = &[
            WindowKind::FixtureSheet,
            WindowKind::SequenceSheet,
            WindowKind::Playbacks,
            WindowKind::CommandLineHistory,
            WindowKind::DmxSheet,
        ];
        const POOLS: &[WindowKind] = &[
            WindowKind::Pools,
            WindowKind::Presets,
            WindowKind::Groups,
            WindowKind::Worlds,
            WindowKind::Filters,
            WindowKind::Effects,
            WindowKind::Timecode,
        ];
        const PLAYBACK: &[WindowKind] = &[
            WindowKind::SequenceSheet,
            WindowKind::Playbacks,
            WindowKind::RunningPlaybacks,
            WindowKind::Masters,
        ];
        const TOOLS: &[WindowKind] = &[
            WindowKind::Patch,
            WindowKind::Status,
            WindowKind::CommandLineHistory,
            WindowKind::DmxSheet,
        ];
        match self {
            WindowTab::Common => COMMON,
            WindowTab::Pools => POOLS,
            WindowTab::Playback => PLAYBACK,
            WindowTab::Tools => TOOLS,
            WindowTab::All => &WindowKind::ALL,
        }
    }
}

/// One placed window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    pub kind: WindowKind,
    pub rect: GridRect,
}

/// The user-defined area. Empty until the operator adds a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    windows: Vec<Window>,
    next_id: u64,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    /// A blank screen.
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
        }
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }

    /// Add a window at the first free place. Returns `None` when the screen
    /// has no rectangle large enough.
    pub fn add(&mut self, kind: WindowKind) -> Option<WindowId> {
        self.add_at(kind, None)
    }

    /// Add a window, preferring `cell` when it can fit there.
    pub fn add_at(&mut self, kind: WindowKind, cell: Option<(u16, u16)>) -> Option<WindowId> {
        let (width, height) = kind.default_size();
        let width = width.min(COLUMNS);
        let height = height.min(ROWS);
        let mut candidates = Vec::new();
        if let Some((x, y)) = cell {
            let x = x.min(COLUMNS.saturating_sub(width));
            let y = y.min(ROWS.saturating_sub(height));
            candidates.push((x, y));
        }
        for y in 0..=ROWS.saturating_sub(height) {
            for x in 0..=COLUMNS.saturating_sub(width) {
                if !candidates.contains(&(x, y)) {
                    candidates.push((x, y));
                }
            }
        }
        let (x, y) = candidates
            .into_iter()
            .find(|(x, y)| self.fits(GridRect::new(*x, *y, width, height), None))?;
        let id = WindowId(self.next_id);
        self.next_id += 1;
        self.windows.push(Window {
            id,
            kind,
            rect: GridRect::new(x, y, width, height),
        });
        Some(id)
    }

    /// Remove one window. The remaining windows keep their IDs and places.
    pub fn remove(&mut self, id: WindowId) -> bool {
        let before = self.windows.len();
        self.windows.retain(|window| window.id != id);
        self.windows.len() != before
    }

    /// Return to the blank screen.
    pub fn clear(&mut self) {
        self.windows.clear();
    }

    /// Move a window to a new cell. Returns false when the rectangle is off
    /// the screen or would overlap another window.
    pub fn move_to(&mut self, id: WindowId, x: u16, y: u16) -> bool {
        let Some(window) = self.windows.iter().find(|window| window.id == id).copied() else {
            return false;
        };
        let rect = GridRect::new(x, y, window.rect.width, window.rect.height);
        if !self.fits(rect, Some(id)) {
            return false;
        }
        if let Some(window) = self.windows.iter_mut().find(|window| window.id == id) {
            window.rect.x = x;
            window.rect.y = y;
            true
        } else {
            false
        }
    }

    /// Resize a window, clamping to the screen edge. Returns false when the
    /// result is below the kind's minimum or overlaps another window.
    pub fn resize(&mut self, id: WindowId, width: u16, height: u16) -> bool {
        let Some(window) = self.windows.iter().find(|window| window.id == id).copied() else {
            return false;
        };
        let (min_width, min_height) = window.kind.min_size();
        let width = width.min(COLUMNS.saturating_sub(window.rect.x));
        let height = height.min(ROWS.saturating_sub(window.rect.y));
        if width < min_width || height < min_height {
            return false;
        }
        let rect = GridRect::new(window.rect.x, window.rect.y, width, height);
        if !self.fits(rect, Some(id)) {
            return false;
        }
        if let Some(window) = self.windows.iter_mut().find(|window| window.id == id) {
            window.rect.width = width;
            window.rect.height = height;
            true
        } else {
            false
        }
    }

    /// The window containing a grid cell, if any.
    pub fn window_at(&self, x: u16, y: u16) -> Option<&Window> {
        self.windows.iter().find(|window| {
            x >= window.rect.x
                && x < window.rect.right()
                && y >= window.rect.y
                && y < window.rect.bottom()
        })
    }

    fn fits(&self, rect: GridRect, ignore: Option<WindowId>) -> bool {
        if rect.width == 0 || rect.height == 0 {
            return false;
        }
        if rect.right() > COLUMNS || rect.bottom() > ROWS {
            return false;
        }
        !self
            .windows
            .iter()
            .any(|window| Some(window.id) != ignore && rect.overlaps(window.rect))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_workspace_is_blank() {
        let workspace = Workspace::new();
        assert!(workspace.is_empty());
        assert!(workspace.windows().is_empty());
        assert_eq!(workspace, Workspace::default());
    }

    #[test]
    fn add_uses_the_first_free_slot_and_unique_ids() {
        let mut workspace = Workspace::new();
        let first = workspace.add(WindowKind::FixtureSheet).unwrap();
        let second = workspace.add(WindowKind::Groups).unwrap();
        assert_ne!(first, second);
        assert_eq!(workspace.windows().len(), 2);
        let first_rect = workspace.get(first).unwrap().rect;
        assert_eq!(first_rect, GridRect::new(0, 0, 8, 5));
        let second_rect = workspace.get(second).unwrap().rect;
        assert_eq!(second_rect, GridRect::new(8, 0, 4, 3));
        assert_eq!(workspace.window_at(0, 0).unwrap().id, first);
        assert!(workspace.window_at(11, 7).is_none());
    }

    #[test]
    fn add_at_prefers_the_clicked_cell_then_falls_back() {
        let mut workspace = Workspace::new();
        let id = workspace.add_at(WindowKind::Groups, Some((4, 3))).unwrap();
        assert_eq!(workspace.get(id).unwrap().rect, GridRect::new(4, 3, 4, 3));

        let second = workspace.add_at(WindowKind::Groups, Some((4, 3))).unwrap();
        assert_ne!(second, id);
        let rect = workspace.get(second).unwrap().rect;
        assert!(!rect.overlaps(workspace.get(id).unwrap().rect));
    }

    #[test]
    fn a_full_screen_refuses_more_windows() {
        let mut workspace = Workspace::new();
        let mut placed = 0;
        while workspace.add(WindowKind::Status).is_some() {
            placed += 1;
            assert!(placed < 100, "placement loop did not terminate");
        }
        assert!(placed > 0);
        assert!(workspace.add(WindowKind::Status).is_none());
    }

    #[test]
    fn move_and_resize_keep_windows_inside_and_apart() {
        let mut workspace = Workspace::new();
        let first = workspace.add(WindowKind::Groups).unwrap();
        let second = workspace.add(WindowKind::Groups).unwrap();
        assert!(!workspace.move_to(second, 0, 0), "overlap must be refused");
        assert!(workspace.move_to(second, 4, 3));
        assert!(!workspace.move_to(second, 11, 0), "outside must be refused");
        assert!(!workspace.resize(second, 0, 0), "minimum must be enforced");
        assert!(workspace.resize(second, 12, 3), "edge must clamp");
        assert_eq!(workspace.get(second).unwrap().rect.width, 8);
        assert!(workspace.resize(second, 4, 3));
        assert_eq!(
            workspace.get(second).unwrap().rect,
            GridRect::new(4, 3, 4, 3)
        );
        assert_eq!(
            workspace.get(first).unwrap().rect,
            GridRect::new(0, 0, 4, 3)
        );
    }

    #[test]
    fn resize_refuses_to_cover_a_neighbour() {
        let mut workspace = Workspace::new();
        let first = workspace.add(WindowKind::Groups).unwrap();
        workspace.add(WindowKind::Groups).unwrap();
        workspace.add(WindowKind::Groups).unwrap();
        let below = workspace.add(WindowKind::Groups).unwrap();
        assert_eq!(
            workspace.get(below).unwrap().rect,
            GridRect::new(0, 3, 4, 3)
        );
        assert!(!workspace.resize(first, 4, 8));
        assert_eq!(
            workspace.get(first).unwrap().rect,
            GridRect::new(0, 0, 4, 3)
        );
    }

    #[test]
    fn resize_clamps_to_the_edge_but_can_reduce_to_the_minimum() {
        let mut workspace = Workspace::new();
        let id = workspace
            .add_at(WindowKind::FixtureSheet, Some((6, 4)))
            .unwrap();
        assert!(workspace.resize(id, 99, 99));
        let rect = workspace.get(id).unwrap().rect;
        assert_eq!(rect.width, 8);
        assert_eq!(rect.height, 5);
        assert!(workspace.resize(id, 4, 3));
        assert!(!workspace.resize(id, 3, 3));
    }

    #[test]
    fn remove_and_clear_keep_the_rest_of_the_screen() {
        let mut workspace = Workspace::new();
        let first = workspace.add(WindowKind::Groups).unwrap();
        let second = workspace.add(WindowKind::Pools).unwrap();
        assert!(workspace.remove(first));
        assert!(!workspace.remove(first));
        assert_eq!(workspace.windows().len(), 1);
        assert_eq!(workspace.windows()[0].id, second);
        workspace.clear();
        assert!(workspace.is_empty());
    }

    #[test]
    fn every_window_has_a_fitting_default_and_minimum() {
        for kind in WindowKind::ALL {
            let (width, height) = kind.default_size();
            let (min_width, min_height) = kind.min_size();
            assert!(width <= COLUMNS && height <= ROWS, "{kind:?}");
            assert!(min_width <= width && min_height <= height, "{kind:?}");
            assert!(min_width > 0 && min_height > 0, "{kind:?}");
        }
    }

    #[test]
    fn every_kind_is_in_all_and_every_tab_is_a_subset() {
        for tab in WindowTab::ALL {
            let kinds = tab.kinds();
            assert!(!kinds.is_empty(), "{tab:?}");
            for kind in kinds {
                assert!(WindowKind::ALL.contains(kind), "{tab:?} contains {kind:?}");
            }
            let mut unique = kinds.to_vec();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), kinds.len(), "{tab:?} has duplicates");
        }
        assert_eq!(WindowTab::All.kinds().len(), WindowKind::ALL.len());
        let titles: Vec<_> = WindowTab::All
            .kinds()
            .iter()
            .map(|kind| kind.title())
            .collect();
        let mut sorted = titles.clone();
        sorted.sort_unstable();
        assert_eq!(titles, sorted, "the All tab is alphabetical");
    }
}
