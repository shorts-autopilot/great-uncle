//! The Add Window picker: category tabs over a grid of catalogue tiles.
//!
//! The picker opens at the clicked point. Choosing a tile places that window at
//! the clicked cell, keeps the picker's layout, and closes it. The height is
//! derived from the tallest tab, so adding a window kind cannot clip a row.

use eframe::egui;
use eframe::egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use openlight_desktop::workspace::{WindowKind, WindowTab};

use super::{Desktop, ACCENT, SCREEN, WINDOW_HEADER};

const PICKER_WIDTH: f32 = 780.0;
const PICKER_COLUMNS: usize = 4;
const TILE_GAP: f32 = 10.0;
const TILE_HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 32.0;
/// Title bar, inner margins and the gap between the tabs and the grid.
const PICKER_CHROME: f32 = 56.0;

impl Desktop {
    /// Draw the picker when it is open and place the chosen kind.
    pub(super) fn add_picker(&mut self, ctx: &egui::Context) {
        if !self.show_add_picker {
            return;
        }
        let mut open = true;
        let screen = ctx.screen_rect();
        let height = picker_height();
        let pos = Pos2::new(
            self.picker_pos.x.clamp(
                screen.left(),
                (screen.right() - PICKER_WIDTH).max(screen.left()),
            ),
            self.picker_pos.y.clamp(
                screen.top(),
                (screen.bottom() - height).max(screen.top()),
            ),
        );
        let mut chosen = None;
        egui::Window::new("Add Window")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_pos(pos)
            .constrain_to(screen)
            .fixed_size([PICKER_WIDTH, height])
            .show(ctx, |ui| {
                self.picker_tabs(ui);
                ui.add_space(10.0);
                chosen = self.picker_grid(ui);
            });
        if let Some(kind) = chosen {
            // The clicked cell is the preferred corner. The workspace clamps it
            // inside the screen edge and falls back to the first free slot if
            // the cell is taken, so no catalogue option can be refused while
            // there is room for it.
            if self.model.add_window_at(kind, self.picker_cell).is_some() {
                self.show_add_picker = false;
            }
        }
        if !open {
            self.show_add_picker = false;
        }
    }

    fn picker_tabs(&mut self, ui: &mut Ui) {
        let tabs = WindowTab::ALL;
        let width = ui.available_width();
        let tab_width = width / tabs.len() as f32;
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal(|ui| {
            for (index, tab) in tabs.iter().enumerate() {
                // The last tab takes the rounding remainder so the strip ends
                // exactly at the popup's right edge.
                let tab_width = if index + 1 == tabs.len() {
                    width - tab_width * index as f32
                } else {
                    tab_width
                };
                let selected = *tab == self.picker_tab;
                let response = ui.add_sized(
                    Vec2::new(tab_width, TAB_HEIGHT),
                    egui::Button::new(RichText::new(tab.title()).size(12.0).color(if selected {
                        ACCENT
                    } else {
                        Color32::from_gray(190)
                    }))
                    .fill(if selected { SCREEN } else { WINDOW_HEADER })
                    .stroke(Stroke::NONE)
                    .corner_radius(0),
                );
                if response.clicked() {
                    self.picker_tab = *tab;
                }
                if selected {
                    ui.painter().rect_filled(
                        Rect::from_min_size(
                            Pos2::new(response.rect.left(), response.rect.bottom() - 2.0),
                            Vec2::new(response.rect.width(), 2.0),
                        ),
                        0.0,
                        ACCENT,
                    );
                }
                if index > 0 {
                    let x = response.rect.left();
                    ui.painter().line_segment(
                        [
                            Pos2::new(x, response.rect.top() + 6.0),
                            Pos2::new(x, response.rect.bottom() - 6.0),
                        ],
                        Stroke::new(1.0_f32, Color32::from_rgb(52, 59, 70)),
                    );
                }
            }
        });
    }

    /// Draw the active tab's tiles and return the kind clicked this frame.
    fn picker_grid(&mut self, ui: &mut Ui) -> Option<WindowKind> {
        let kinds = self.picker_tab.kinds();
        let width = ui.available_width();
        let tile_width = (width - TILE_GAP * (PICKER_COLUMNS - 1) as f32) / PICKER_COLUMNS as f32;
        let rows = kinds.len().div_ceil(PICKER_COLUMNS);
        let height = rows as f32 * TILE_HEIGHT + rows.saturating_sub(1) as f32 * TILE_GAP;
        let (grid, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let mut chosen = None;

        for (index, kind) in kinds.iter().enumerate() {
            let row = index / PICKER_COLUMNS;
            let column = index % PICKER_COLUMNS;
            let rect = Rect::from_min_size(
                grid.min
                    + Vec2::new(
                        column as f32 * (tile_width + TILE_GAP),
                        row as f32 * (TILE_HEIGHT + TILE_GAP),
                    ),
                Vec2::new(tile_width, TILE_HEIGHT),
            );
            let response = ui
                .interact(
                    rect,
                    ui.id().with(("add-window-tile", kind.title())),
                    Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            paint_tile(ui, *kind, rect, response.hovered());
            if response.clicked() {
                chosen = Some(*kind);
            }
        }
        chosen
    }
}

/// The popup height: the tallest tab decides how many rows the grid needs.
fn picker_height() -> f32 {
    let rows = WindowTab::ALL
        .iter()
        .map(|tab| tab.kinds().len().div_ceil(PICKER_COLUMNS))
        .max()
        .unwrap_or(1);
    let grid = rows as f32 * TILE_HEIGHT + rows.saturating_sub(1) as f32 * TILE_GAP;
    TAB_HEIGHT + 10.0 + grid + PICKER_CHROME
}

/// One catalogue tile: an empty placeholder box inline with the window name.
fn paint_tile(ui: &Ui, kind: WindowKind, rect: Rect, hovered: bool) {
    let painter = ui.painter();
    let fill = if hovered {
        Color32::from_rgb(40, 46, 56)
    } else {
        WINDOW_HEADER
    };
    let border = if hovered {
        ACCENT
    } else {
        Color32::from_rgb(52, 59, 70)
    };
    painter.rect_filled(rect, 3, fill);
    painter.rect_stroke(
        rect,
        3,
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );

    // The placeholder is square and exactly one line high, centred with the
    // label so it can never stretch the tile.
    let font = FontId::monospace(11.0);
    let row_height = ui.fonts(|fonts| fonts.row_height(&font));
    let placeholder = Rect::from_center_size(
        Pos2::new(rect.left() + 10.0 + row_height / 2.0, rect.center().y),
        Vec2::splat(row_height),
    );
    painter.rect_filled(placeholder, 2, Color32::from_rgb(45, 50, 60));
    painter.rect_stroke(
        placeholder,
        2,
        Stroke::new(1.0_f32, Color32::from_rgb(70, 78, 90)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        Pos2::new(placeholder.right() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        kind.title(),
        font,
        Color32::from_rgb(222, 226, 233),
    );
}
