//! The eframe window: a user-defined screen of movable windows.
//!
//! The screen, its 12 × 8 dotted cell grid and any placed window are drawn. A
//! click on empty canvas opens the Add Window picker at that point; choosing a
//! catalogue tile places that window at the clicked cell and closes the
//! picker. Each window draws the listing rows the engine already produces, and
//! every control becomes command text through [`Model`], so a pane can never
//! mutate the show directly.

use std::time::{Duration, Instant};

use eframe::egui;
use openlight_app::{format_cue_number, App, PlaybackRow, Snapshot, TimecodeRowKind};
use openlight_desktop::model::{delete_command, pool_command, step_percent};
use openlight_desktop::workspace::{
    GridRect, Window, WindowId, WindowKind, WindowTab, COLUMNS, ROWS,
};
use openlight_desktop::Model;

use eframe::egui::{
    Align, Align2, Color32, FontId, Key, Label, Layout, Margin, Pos2, Rect, RichText, ScrollArea,
    Sense, Stroke, TextEdit, TextStyle, Ui, UiBuilder, Vec2,
};

const SCREEN: Color32 = Color32::from_rgb(9, 11, 15);
const WINDOW: Color32 = Color32::from_rgb(24, 28, 35);
const WINDOW_HEADER: Color32 = Color32::from_rgb(32, 37, 46);
const GRID_LINE: Color32 = Color32::from_rgb(38, 43, 52);
const ACCENT: Color32 = Color32::from_rgb(225, 165, 72);
const MUTED: Color32 = Color32::from_rgb(150, 157, 168);
const DANGER: Color32 = Color32::from_rgb(229, 94, 94);
const OK: Color32 = Color32::from_rgb(111, 190, 139);

const PICKER_WIDTH: f32 = 780.0;
const PICKER_HEIGHT: f32 = 300.0;
const PICKER_COLUMNS: usize = 4;
const TILE_GAP: f32 = 10.0;
const TILE_HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 32.0;

/// A snapshot at least every 250 ms even when nothing changed, so the rendered
/// frame and the status counters stay live.
const SNAPSHOT_FLOOR: Duration = Duration::from_millis(250);
/// One executor command at most every 33 ms while a fader is dragged.
const FADER_INTERVAL: Duration = Duration::from_millis(33);

pub fn run(app: App) -> Result<(), String> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OpenLight")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "OpenLight",
        native_options,
        Box::new(|cc| Ok(Box::new(Desktop::new(app, cc)))),
    )
    .map_err(|error| error.to_string())
}

#[derive(Clone, Copy)]
struct DragState {
    id: WindowId,
    start_pointer: Pos2,
    start: GridRect,
}

#[derive(Clone, Copy)]
struct ResizeState {
    id: WindowId,
    start_pointer: Pos2,
    start: GridRect,
}

struct Desktop {
    app: App,
    model: Model,
    /// One owned copy per repaint; drawing never holds the engine lock.
    snapshot: Snapshot,
    last_snapshot: Instant,
    last_fader_send: Instant,
    /// `(executor, 0..=1)`, latest value wins while a drag is throttled.
    pending_fader: Option<(u16, f64)>,
    /// Fader positions as the operator left them, 0..=100.
    faders: [f64; 9],
    grand: f64,
    /// Level the Fixture Sheet's `At` button applies to the selection.
    at_level: f64,
    /// Attribute and value the sheet's `Def` button stores when both are set;
    /// an empty pair makes `Def` list the effective home instead.
    default_attribute: String,
    default_value: String,
    /// Position the timecode `Locate` and `Add` buttons send.
    timecode_at: String,
    /// Event text for the timecode `Add` button, e.g. `Sequence 1 Cue 1`.
    timecode_event: String,
    /// Focus the Command Line History input on the next frame.
    focus_command: bool,
    show_add_picker: bool,
    picker_pos: Pos2,
    picker_cell: Option<(u16, u16)>,
    picker_tab: WindowTab,
    drag: Option<DragState>,
    resize: Option<ResizeState>,
}

impl Desktop {
    fn new(app: App, cc: &eframe::CreationContext<'_>) -> Self {
        apply_theme(&cc.egui_ctx);

        let snapshot = app.snapshot();
        let mut faders = [100.0; 9];
        for view in &snapshot.desk.executors {
            if let Some(slot) = faders.get_mut(usize::from(view.number.saturating_sub(1))) {
                *slot = view.fader * 100.0;
            }
        }
        let grand = snapshot.desk.grand * 100.0;
        Self {
            app,
            model: Model::new(),
            snapshot,
            last_snapshot: Instant::now(),
            last_fader_send: Instant::now(),
            pending_fader: None,
            faders,
            grand,
            at_level: 100.0,
            default_attribute: String::new(),
            default_value: String::new(),
            timecode_at: "00:00:00:00".into(),
            timecode_event: String::new(),
            focus_command: false,
            show_add_picker: false,
            picker_pos: Pos2::ZERO,
            picker_cell: None,
            picker_tab: WindowTab::Common,
            drag: None,
            resize: None,
        }
    }

    fn act(&mut self, command: &str) {
        let _ = self.model.act(&self.app, command);
    }

    /// Copy the engine's master and executor positions back into the widgets.
    fn sync_faders(&mut self) {
        self.grand = self.snapshot.desk.grand * 100.0;
        for view in &self.snapshot.desk.executors {
            if let Some(slot) = self
                .faders
                .get_mut(usize::from(view.number.saturating_sub(1)))
            {
                *slot = view.fader * 100.0;
            }
        }
    }

    fn flush_fader(&mut self, ctx: &egui::Context) {
        let Some((executor, value)) = self.pending_fader else {
            return;
        };
        if self.last_fader_send.elapsed() < FADER_INTERVAL {
            ctx.request_repaint_after(FADER_INTERVAL);
            return;
        }
        let _ = self.model.drag_fader(&self.app, executor, value);
        self.last_fader_send = Instant::now();
        self.pending_fader = None;
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.show_add_picker = false;
        }
    }

    fn workspace_panel(&mut self, ctx: &egui::Context) {
        let mut open_picker_at = None;
        // No panel margin: the grid reaches the window edge and a click in the
        // corner is still a click on the canvas.
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ctx, |ui| {
                let screen = ui.max_rect();
                let cell = Vec2::new(
                    (screen.width() / f32::from(COLUMNS)).max(1.0),
                    (screen.height() / f32::from(ROWS)).max(1.0),
                );
                ui.painter().rect_filled(screen, 0.0, SCREEN);
                paint_grid(ui, screen, cell);

                if self.model.workspace.is_empty() {
                    ui.painter().text(
                        screen.center() - Vec2::new(0.0, 12.0),
                        Align2::CENTER_CENTER,
                        "SCREEN 1 IS EMPTY",
                        FontId::monospace(22.0),
                        MUTED,
                    );
                    ui.painter().text(
                        screen.center() + Vec2::new(0.0, 18.0),
                        Align2::CENTER_CENTER,
                        "Add windows to build a workspace",
                        FontId::monospace(13.0),
                        Color32::from_gray(110),
                    );
                }

                let windows = self.model.workspace.windows().to_vec();

                // The click is tested before anything is drawn, while every
                // placed window is still in the workspace. The close button
                // removes its window during the frame, so testing afterwards
                // reads the cell as empty and opens the picker under the
                // pointer that just closed a pane.
                let clicked = ui.input(|input| input.pointer.primary_clicked());
                if clicked && !self.show_add_picker {
                    if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                        if screen.contains(pointer) {
                            if let Some((x, y)) = grid_cell(screen, cell, pointer) {
                                if self.model.workspace.window_at(x, y).is_none() {
                                    open_picker_at = Some((pointer, (x, y)));
                                }
                            }
                        }
                    }
                }

                for window in windows {
                    self.draw_window(ui, &window, screen, cell);
                }
                self.apply_layout_gestures(ctx, cell);
            });
        if let Some((pos, cell)) = open_picker_at {
            self.show_add_picker = true;
            self.picker_pos = pos;
            self.picker_cell = Some(cell);
            self.picker_tab = WindowTab::Common;
        }
        self.add_picker(ctx);
    }

    /// The Add Window picker: category tabs over the current catalogue. It
    /// opens at the clicked point; choosing a tile places that window at the
    /// clicked cell, keeps the picker's layout, and closes it.
    fn add_picker(&mut self, ctx: &egui::Context) {
        if !self.show_add_picker {
            return;
        }
        let mut open = true;
        let screen = ctx.screen_rect();
        let pos = Pos2::new(
            self.picker_pos.x.clamp(
                screen.left(),
                (screen.right() - PICKER_WIDTH).max(screen.left()),
            ),
            self.picker_pos.y.clamp(
                screen.top(),
                (screen.bottom() - PICKER_HEIGHT).max(screen.top()),
            ),
        );
        let mut chosen = None;
        egui::Window::new("Add Window")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_pos(pos)
            .constrain_to(screen)
            .fixed_size([PICKER_WIDTH, PICKER_HEIGHT])
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

    fn draw_window(&mut self, ui: &mut Ui, window: &Window, screen: Rect, cell: Vec2) {
        let window_rect = Rect::from_min_size(
            screen.min
                + Vec2::new(
                    f32::from(window.rect.x) * cell.x,
                    f32::from(window.rect.y) * cell.y,
                ),
            Vec2::new(
                f32::from(window.rect.width) * cell.x,
                f32::from(window.rect.height) * cell.y,
            ),
        )
        .shrink(2.0);
        if window_rect.width() < 8.0 || window_rect.height() < 8.0 {
            return;
        }

        let mut child = ui.new_child(UiBuilder::new().max_rect(window_rect));
        child.set_clip_rect(window_rect);
        let mut close = false;
        let mut drag_start = None;
        let mut resize_start = None;

        egui::Frame::new()
            .fill(WINDOW)
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(52, 59, 70)))
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(Margin::same(4))
            .show(&mut child, |ui| {
                ui.set_clip_rect(window_rect);
                // The whole title bar is the drag handle; the close button is
                // created afterwards, so it wins the corner hit test.
                let header_rect =
                    Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 20.0));
                ui.painter()
                    .rect_filled(header_rect, egui::CornerRadius::same(2), WINDOW_HEADER);
                let header = ui.interact(
                    header_rect,
                    ui.id().with(("window-drag", window.id.0)),
                    Sense::click_and_drag(),
                );
                if header.drag_started() {
                    drag_start = header.interact_pointer_pos();
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(window.kind.title())
                            .strong()
                            .color(Color32::from_rgb(222, 226, 233)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button(RichText::new("×").color(DANGER))
                            .on_hover_text("Close window")
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
                ui.separator();
                ui.push_id(window.id, |ui| {
                    self.window_body(window.kind, ui);
                });

                // The grip is created after the body so it wins hit testing in
                // the lower-right corner.
                let grip =
                    Rect::from_min_size(window_rect.max - Vec2::splat(14.0), Vec2::splat(14.0));
                let response = ui.interact(
                    grip,
                    ui.id().with(("window-resize", window.id.0)),
                    Sense::drag(),
                );
                ui.painter().line_segment(
                    [
                        grip.right_bottom() - Vec2::new(2.0, 10.0),
                        grip.right_bottom() - Vec2::new(10.0, 2.0),
                    ],
                    Stroke::new(1.0_f32, MUTED),
                );
                if response.drag_started() {
                    resize_start = response.interact_pointer_pos();
                }
                ui.painter().rect_stroke(
                    grip,
                    0.0,
                    Stroke::new(0.5_f32, Color32::from_rgb(70, 78, 90)),
                    egui::StrokeKind::Inside,
                );
            });

        if let Some(pointer) = drag_start {
            self.drag = Some(DragState {
                id: window.id,
                start_pointer: pointer,
                start: window.rect,
            });
            self.resize = None;
        }
        if let Some(pointer) = resize_start {
            self.resize = Some(ResizeState {
                id: window.id,
                start_pointer: pointer,
                start: window.rect,
            });
            self.drag = None;
        }
        if close {
            self.model.remove_window(window.id);
            if self.drag.is_some_and(|drag| drag.id == window.id) {
                self.drag = None;
            }
            if self.resize.is_some_and(|resize| resize.id == window.id) {
                self.resize = None;
            }
        }
    }

    /// Draw the pane for one window kind from the current snapshot.
    fn window_body(&mut self, kind: WindowKind, ui: &mut Ui) {
        match kind {
            WindowKind::FixtureSheet => self.sheet(ui),
            WindowKind::SequenceSheet => self.playback(ui),
            WindowKind::Playbacks => self.playbacks(ui),
            WindowKind::Pools => self.pools(ui, None),
            WindowKind::Presets => self.pools(ui, Some("Palette")),
            WindowKind::Groups => self.pools(ui, Some("Group")),
            WindowKind::Worlds => self.pools(ui, Some("World")),
            WindowKind::Filters => self.pools(ui, Some("Filter")),
            WindowKind::Effects => self.pools(ui, Some("Effect")),
            WindowKind::Timecode => self.timecode(ui),
            WindowKind::CommandLineHistory => self.command_history(ui),
            WindowKind::DmxSheet => self.dmx_sheet(ui),
            WindowKind::Masters => self.masters(ui),
            WindowKind::RunningPlaybacks => self.running_playbacks(ui),
            WindowKind::Patch => self.patch(ui),
            WindowKind::Status => self.status(ui),
        }
    }

    fn sheet(&mut self, ui: &mut Ui) {
        let rows = &self.snapshot.listings.sheet;
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("Selection:");
            ui.add(egui::Slider::new(&mut self.at_level, 0.0..=100.0).text("%"));
            if ui.button("At").clicked() {
                action = Some(format!("At {}", self.at_level.round() as i64));
            }
            ui.label("Default:");
            ui.add(
                TextEdit::singleline(&mut self.default_attribute)
                    .desired_width(70.0)
                    .hint_text("attr"),
            );
            ui.add(
                TextEdit::singleline(&mut self.default_value)
                    .desired_width(50.0)
                    .hint_text("value"),
            );
        });
        let default_attribute = self.default_attribute.trim().to_string();
        let default_value = self.default_value.trim().to_string();
        if rows.is_empty() {
            ui.label("No fixtures patched.");
            return;
        }
        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("sheet")
                    .num_columns(8)
                    .striped(true)
                    .spacing([12.0, 3.0])
                    .show(ui, |ui| {
                        for header in [
                            "", "No.", "Name", "Type", "Mode", "Patch", "Output", "Colour",
                        ] {
                            ui.monospace(RichText::new(header).strong());
                        }
                        ui.end_row();
                        for row in rows {
                            ui.horizontal(|ui| {
                                if ui.small_button("Sel").clicked() {
                                    action = Some(format!("Fixture {}", row.number));
                                }
                                if ui.small_button("Home").clicked() {
                                    action = Some(format!("Fixture {} Home", row.number));
                                }
                                if ui.small_button("Def").clicked() {
                                    action = Some(if default_attribute.is_empty()
                                        || default_value.is_empty()
                                    {
                                        format!("Fixture {} Default", row.number)
                                    } else {
                                        format!(
                                            "Fixture {} Default {default_attribute} {default_value}",
                                            row.number
                                        )
                                    });
                                }
                            });
                            ui.monospace(row.number.to_string());
                            ui.monospace(&row.name);
                            ui.monospace(&row.type_key);
                            ui.monospace(&row.mode);
                            ui.monospace(row.patch());
                            ui.monospace(row.output());
                            match row.colour {
                                Some([red, green, blue]) => {
                                    let (rect, _) =
                                        ui.allocate_exact_size(egui::vec2(16.0, 14.0), Sense::hover());
                                    ui.painter().rect_filled(
                                        rect,
                                        2.0,
                                        Color32::from_rgb(red, green, blue),
                                    );
                                }
                                None => {
                                    ui.monospace("-");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(action) = action {
            self.act(&action);
        }
    }

    fn playback(&mut self, ui: &mut Ui) {
        let rows = &self.snapshot.listings.playback;
        if rows.is_empty() {
            ui.label("No cues stored.");
            return;
        }
        let selected = self.model.selected_sequence;
        let mut select = None;
        let mut transport = None;
        if let Some(sequence) = selected {
            ui.horizontal_wrapped(|ui| {
                ui.monospace(format!("Sequence {sequence}"));
                for action in ["Go", "Back", "Pause", "Release"] {
                    if ui.button(action).clicked() {
                        transport = Some(action);
                    }
                }
            });
        } else {
            ui.label("Click a cue row to choose its sequence.");
        }
        let mut clicked = None;
        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("playback")
                    .num_columns(10)
                    .striped(true)
                    .spacing([12.0, 3.0])
                    .show(ui, |ui| {
                        for header in [
                            "", "Seq", "Cue", "Name", "Trigger", "Fade", "Delay", "Mib", "Values",
                            "State",
                        ] {
                            ui.monospace(RichText::new(header).strong());
                        }
                        ui.end_row();
                        for row in rows {
                            ui.horizontal(|ui| {
                                if ui.small_button("Sel").clicked() {
                                    select = Some(row.sequence);
                                }
                                if ui.small_button("Go").clicked() {
                                    clicked = Some(row.clone());
                                }
                            });
                            ui.monospace(row.sequence.to_string());
                            ui.monospace(format_cue_number(row.cue));
                            ui.monospace(&row.cue_name);
                            ui.monospace(&row.trigger);
                            ui.monospace(format!("{:.1}s", row.fade));
                            ui.monospace(format!("{:.1}s", row.delay));
                            ui.monospace(if row.mib { "yes" } else { "-" });
                            ui.monospace(row.values.to_string());
                            ui.monospace(state(row));
                            ui.end_row();
                        }
                    });
            });
        if let Some(sequence) = select {
            self.model.select_sequence(sequence);
        }
        if let Some(row) = clicked {
            let _ = self.model.click_cue(&self.app, &row);
        } else if let Some(action) = transport {
            let _ = self.model.playback(&self.app, action);
        }
    }

    fn pools(&mut self, ui: &mut Ui, filter: Option<&str>) {
        let rows: Vec<_> = self
            .snapshot
            .listings
            .pools
            .iter()
            .filter(|row| filter.is_none_or(|kind| row.kind == kind))
            .collect();
        if rows.is_empty() {
            ui.label(if let Some(kind) = filter {
                format!("No {kind} entries stored.")
            } else {
                "No palettes, groups, worlds, filters or effects stored.".to_string()
            });
            return;
        }
        let mut action = None;
        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("pools")
                    .num_columns(6)
                    .striped(true)
                    .spacing([12.0, 3.0])
                    .show(ui, |ui| {
                        for header in ["", "", "Kind", "No.", "Name", "Detail"] {
                            ui.monospace(RichText::new(header).strong());
                        }
                        ui.end_row();
                        for row in rows {
                            if ui
                                .small_button(if row.active { "Off" } else { "Use" })
                                .clicked()
                            {
                                action = Some(pool_command(
                                    &row.kind,
                                    row.number,
                                    &row.detail,
                                    row.active,
                                ));
                            }
                            if ui.small_button("Del").clicked() {
                                action = Some(delete_command(&row.kind, row.number, &row.detail));
                            }
                            ui.monospace(&row.kind);
                            ui.monospace(row.number.to_string());
                            ui.monospace(&row.name);
                            ui.monospace(&row.detail);
                            ui.end_row();
                        }
                    });
            });
        if let Some(action) = action {
            self.act(&action);
        }
    }

    fn timecode(&mut self, ui: &mut Ui) {
        let rows = &self.snapshot.listings.timecode;
        if rows.is_empty() {
            ui.label("No timecode shows stored.");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Locate:");
            ui.add(
                TextEdit::singleline(&mut self.timecode_at)
                    .desired_width(110.0)
                    .font(TextStyle::Monospace),
            );
            ui.label("Event:");
            ui.add(
                TextEdit::singleline(&mut self.timecode_event)
                    .desired_width(190.0)
                    .hint_text("Sequence 1 Cue 1"),
            );
        });
        let at = self.timecode_at.trim().to_string();
        let event = self.timecode_event.trim().to_string();
        let mut action = None;
        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("timecode")
                    .num_columns(7)
                    .striped(true)
                    .spacing([12.0, 3.0])
                    .show(ui, |ui| {
                        for header in ["", "TC", "Kind", "Track", "Time", "Name", "Action"] {
                            ui.monospace(RichText::new(header).strong());
                        }
                        ui.end_row();
                        for row in rows {
                            if row.kind == TimecodeRowKind::Show {
                                ui.horizontal(|ui| {
                                    if ui.small_button("Start").clicked() {
                                        action = Some(format!("Timecode {} Start", row.timecode));
                                    }
                                    if ui.small_button("Pause").clicked() {
                                        action = Some(format!("Timecode {} Pause", row.timecode));
                                    }
                                    if ui.small_button("Stop").clicked() {
                                        action = Some(format!("Timecode {} Stop", row.timecode));
                                    }
                                    if !at.is_empty() && ui.small_button("Locate").clicked() {
                                        action = Some(format!("Timecode {} At {at}", row.timecode));
                                    }
                                    if !at.is_empty()
                                        && !event.is_empty()
                                        && ui.small_button("Add").clicked()
                                    {
                                        action = Some(format!(
                                            "Timecode {} Add {at} {event}",
                                            row.timecode
                                        ));
                                    }
                                });
                            } else {
                                ui.label("");
                            }
                            ui.monospace(row.timecode.to_string());
                            ui.monospace(row.kind.label());
                            ui.monospace(if row.track == 0 {
                                "-".to_string()
                            } else {
                                row.track.to_string()
                            });
                            ui.monospace(&row.at);
                            ui.monospace(&row.name);
                            ui.monospace(&row.action);
                            ui.end_row();
                        }
                    });
            });
        if let Some(action) = action {
            self.act(&action);
        }
    }

    fn playbacks(&mut self, ui: &mut Ui) {
        let mut pending = None;
        let mut flash_action = None;
        let has_executor: [bool; 9] = std::array::from_fn(|index| {
            self.snapshot
                .desk
                .executors
                .iter()
                .any(|view| usize::from(view.number) == index + 1)
        });
        let flash: [bool; 9] = std::array::from_fn(|index| {
            self.snapshot
                .desk
                .executors
                .iter()
                .find(|view| usize::from(view.number) == index + 1)
                .map(|view| view.flash)
                .unwrap_or(false)
        });
        ui.monospace(format!(
            "FADERS  ·  page {}  {}",
            self.snapshot.desk.page, self.snapshot.desk.page_name
        ));
        ScrollArea::horizontal()
            .id_salt("playback-faders")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().slider_width = 48.0;
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.horizontal(|ui| {
                    for number in 1..=9 {
                        ui.vertical(|ui| {
                            ui.monospace(format!("{number}"));
                            let enabled = has_executor[number - 1];
                            let before = self.faders[number - 1];
                            let response = ui.add_enabled(
                                enabled,
                                egui::Slider::new(&mut self.faders[number - 1], 0.0..=100.0)
                                    .vertical()
                                    .show_value(true),
                            );
                            if response.changed() {
                                pending = Some((number as u16, self.faders[number - 1] / 100.0));
                            }
                            if response.has_focus() {
                                let step = step_from_keys(ui);
                                if step != 0.0 {
                                    self.faders[number - 1] = step_percent(before, step);
                                    pending =
                                        Some((number as u16, self.faders[number - 1] / 100.0));
                                }
                            }
                            if enabled
                                && ui
                                    .selectable_label(flash[number - 1], RichText::new("F").small())
                                    .on_hover_text("Flash")
                                    .clicked()
                            {
                                flash_action = Some(if flash[number - 1] {
                                    format!("Executor {number} Flash Off")
                                } else {
                                    format!("Executor {number} Flash")
                                });
                            }
                        });
                    }
                });
            });
        ui.separator();
        ui.monospace("GRAND");
        let before = self.grand;
        let response = ui.add(
            egui::Slider::new(&mut self.grand, 0.0..=100.0)
                .text("grand master")
                .show_value(true),
        );
        if response.changed() {
            pending = Some((0, self.grand / 100.0));
        }
        if response.has_focus() {
            let step = step_from_keys(ui);
            if step != 0.0 {
                self.grand = step_percent(before, step);
                pending = Some((0, self.grand / 100.0));
            }
        }
        ui.add_space(4.0);
        let running = self.snapshot.playing.len();
        ui.monospace(format!("{running} playback(s) running"));
        if pending.is_some() {
            self.pending_fader = pending;
        }
        if let Some(action) = flash_action {
            self.act(&action);
        }
    }

    fn masters(&mut self, ui: &mut Ui) {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Page Prev").clicked() {
                action = Some("Page Prev".to_string());
            }
            if ui.button("Page Next").clicked() {
                action = Some("Page Next".to_string());
            }
        });
        ui.separator();
        ui.monospace(format!(
            "Page      {}  {}",
            self.snapshot.desk.page, self.snapshot.desk.page_name
        ));
        ui.monospace(format!(
            "Grand     {:.0}%",
            self.snapshot.desk.grand * 100.0
        ));
        ui.monospace(format!("Rate      {:.0}%", self.snapshot.desk.rate * 100.0));
        ui.monospace(format!("Tempo     {:.1} BPM", self.snapshot.desk.tempo));
        ui.monospace(format!("Selected  {}", self.snapshot.desk.selection.len()));
        ui.monospace(format!("Blind     {}", yes_no(self.snapshot.desk.blind)));
        ui.monospace(format!("Blackout  {}", yes_no(self.snapshot.desk.blackout)));
        if let Some(action) = action {
            self.act(&action);
        }
    }

    fn running_playbacks(&mut self, ui: &mut Ui) {
        if self.snapshot.playing.is_empty() {
            ui.label("No playback is running.");
            return;
        }
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for playback in &self.snapshot.playing {
                    ui.horizontal(|ui| {
                        ui.monospace(format!("{:<18}", playback.target));
                        ui.monospace(format!("{:<10}", playback.cue));
                        ui.monospace(format!("{:>4.0}%", playback.progress * 100.0));
                        let label = if playback.done {
                            "complete".to_string()
                        } else if playback.paused {
                            "paused".to_string()
                        } else {
                            "running".to_string()
                        };
                        ui.label(if playback.done || playback.paused {
                            RichText::new(label).color(MUTED)
                        } else {
                            RichText::new(label).color(OK)
                        });
                    });
                }
            });
    }

    fn dmx_sheet(&mut self, ui: &mut Ui) {
        let universes: Vec<(u16, Vec<u8>)> = self
            .snapshot
            .frame
            .universes()
            .map(|(number, slots)| (number, slots.to_vec()))
            .collect();
        if universes.is_empty() {
            ui.label("No rendered universes yet.");
            return;
        }
        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (universe, slots) in universes {
                    ui.monospace(
                        RichText::new(format!("Universe {universe}"))
                            .strong()
                            .color(ACCENT),
                    );
                    egui::Grid::new(format!("dmx-{universe}"))
                        .num_columns(16)
                        .striped(true)
                        .spacing([5.0, 1.0])
                        .show(ui, |ui| {
                            for address in 1..=slots.len() {
                                let value = slots[address - 1];
                                ui.monospace(
                                    RichText::new(format!("{address:03}:{value:03}"))
                                        .small()
                                        .color(if value == 0 {
                                            MUTED
                                        } else {
                                            Color32::from_gray(220)
                                        }),
                                );
                                if address % 16 == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                    ui.add_space(6.0);
                }
            });
    }

    fn patch(&mut self, ui: &mut Ui) {
        let rows = &self.snapshot.listings.sheet;
        if rows.is_empty() {
            ui.label("No fixtures patched.");
            return;
        }
        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("patch")
                    .num_columns(5)
                    .striped(true)
                    .spacing([12.0, 3.0])
                    .show(ui, |ui| {
                        for header in ["No.", "Name", "Type", "Mode", "Patch"] {
                            ui.monospace(RichText::new(header).strong());
                        }
                        ui.end_row();
                        for row in rows {
                            ui.monospace(row.number.to_string());
                            ui.monospace(&row.name);
                            ui.monospace(&row.type_key);
                            ui.monospace(&row.mode);
                            ui.monospace(row.patch());
                            ui.end_row();
                        }
                    });
            });
    }

    fn command_history(&mut self, ui: &mut Ui) {
        let mut submit = false;
        ui.horizontal(|ui| {
            ui.monospace(RichText::new("openlight>").color(ACCENT));
            let response = ui.add(
                TextEdit::singleline(&mut self.model.input)
                    .desired_width(f32::INFINITY)
                    .font(TextStyle::Monospace)
                    .hint_text("command"),
            );
            if self.focus_command {
                response.request_focus();
                self.focus_command = false;
            }
            submit = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        });
        if submit {
            let _ = self.model.submit(&self.app);
            self.focus_command = true;
        }
        let colour = if self.model.error { DANGER } else { MUTED };
        ui.monospace(RichText::new(&self.model.message).color(colour));
        if let Some(echo) = self.model.last_echo() {
            ui.monospace(RichText::new(format!("> {echo}")).color(Color32::from_gray(140)));
        }
        ui.separator();
        if self.model.history.is_empty() {
            ui.label("No commands run yet.");
            return;
        }
        let history = self.model.history.clone();
        let mut recall = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &history {
                    let command = ui.add(
                        Label::new(
                            RichText::new(format!("> {}", entry.command))
                                .monospace()
                                .color(if entry.error {
                                    DANGER
                                } else {
                                    Color32::from_gray(220)
                                }),
                        )
                        .sense(Sense::click()),
                    );
                    if command.clicked() {
                        recall = Some(entry.command.clone());
                    }
                    command.on_hover_text("Click to recall this command");
                    ui.label(RichText::new(&entry.message).small().color(if entry.error {
                        DANGER
                    } else {
                        MUTED
                    }));
                    ui.separator();
                }
            });
        if let Some(command) = recall {
            self.model.input = command;
            self.focus_command = true;
        }
    }

    fn status(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in &self.snapshot.listings.status {
                    ui.monospace(line);
                }
            });
    }

    fn apply_layout_gestures(&mut self, ctx: &egui::Context, cell: Vec2) {
        let pointer = ctx.pointer_interact_pos();
        let down = ctx.input(|input| input.pointer.primary_down());

        if let Some(drag) = self.drag {
            if let Some(pointer) = pointer {
                let dx = ((pointer.x - drag.start_pointer.x) / cell.x).round() as i32;
                let dy = ((pointer.y - drag.start_pointer.y) / cell.y).round() as i32;
                let max_x = i32::from(COLUMNS).saturating_sub(i32::from(drag.start.width));
                let max_y = i32::from(ROWS).saturating_sub(i32::from(drag.start.height));
                let x = (i32::from(drag.start.x) + dx).clamp(0, max_x.max(0)) as u16;
                let y = (i32::from(drag.start.y) + dy).clamp(0, max_y.max(0)) as u16;
                let _ = self.model.workspace.move_to(drag.id, x, y);
            }
            if !down {
                self.drag = None;
            }
        }

        if let Some(resize) = self.resize {
            if let Some(pointer) = pointer {
                let dx = ((pointer.x - resize.start_pointer.x) / cell.x).round() as i32;
                let dy = ((pointer.y - resize.start_pointer.y) / cell.y).round() as i32;
                let width = (i32::from(resize.start.width) + dx).max(1) as u16;
                let height = (i32::from(resize.start.height) + dy).max(1) as u16;
                let _ = self.model.workspace.resize(resize.id, width, height);
            }
            if !down {
                self.resize = None;
            }
        }
    }
}

impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.keys(ctx);
        // Rebuild at most every frame from the revision counter, and at least
        // every 250 ms so the DMX sheet and status counters stay live.
        let dirty = self.model.take_dirty();
        let revision = self.app.revision();
        if dirty
            || revision != self.snapshot.revision
            || self.last_snapshot.elapsed() >= SNAPSHOT_FLOOR
        {
            self.snapshot = self.app.snapshot();
            self.last_snapshot = Instant::now();
            if self.pending_fader.is_none() {
                self.sync_faders();
            }
        }
        self.workspace_panel(ctx);
        self.flush_fader(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.stop();
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = SCREEN;
    visuals.window_fill = WINDOW;
    visuals.faint_bg_color = WINDOW_HEADER;
    visuals.selection.bg_fill = Color32::from_rgb(55, 79, 98);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style
        .text_styles
        .insert(TextStyle::Body, FontId::monospace(13.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(13.0));
    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    ctx.set_style(style);
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

/// Paint the cell edges as dots, so the lines show exactly where a window can
/// sit.
fn paint_grid(ui: &Ui, screen: Rect, cell: Vec2) {
    for column in 0..=COLUMNS {
        let x = screen.left() + f32::from(column) * cell.x;
        dotted_line(
            ui,
            Pos2::new(x, screen.top()),
            Pos2::new(x, screen.bottom()),
        );
    }
    for row in 0..=ROWS {
        let y = screen.top() + f32::from(row) * cell.y;
        dotted_line(
            ui,
            Pos2::new(screen.left(), y),
            Pos2::new(screen.right(), y),
        );
    }
}

/// A line of dots, spaced so it reads as a guide rather than a border.
fn dotted_line(ui: &Ui, from: Pos2, to: Pos2) {
    let delta = to - from;
    let length = delta.length();
    if length <= 0.0 {
        return;
    }
    let direction = delta / length;
    let mut distance = 0.0;
    while distance <= length {
        ui.painter()
            .circle_filled(from + direction * distance, 1.0, GRID_LINE);
        distance += 8.0;
    }
}

fn grid_cell(screen: Rect, cell: Vec2, pointer: Pos2) -> Option<(u16, u16)> {
    let x = ((pointer.x - screen.left()) / cell.x).floor();
    let y = ((pointer.y - screen.top()) / cell.y).floor();
    if x < 0.0 || y < 0.0 || x >= f32::from(COLUMNS) || y >= f32::from(ROWS) {
        return None;
    }
    Some((x as u16, y as u16))
}

/// A 5% keyboard nudge while a fader has focus.
fn step_from_keys(ui: &Ui) -> f64 {
    ui.input(|input| {
        if input.key_pressed(Key::ArrowUp) || input.key_pressed(Key::ArrowRight) {
            5.0
        } else if input.key_pressed(Key::ArrowDown) || input.key_pressed(Key::ArrowLeft) {
            -5.0
        } else {
            0.0
        }
    })
}

/// How a cue row's playback reads on the sheet.
fn state(row: &PlaybackRow) -> String {
    match &row.running {
        None => "-".to_string(),
        Some(run) if run.paused => "paused".to_string(),
        Some(run) if run.done => "complete".to_string(),
        Some(run) => format!("{:.0}%", run.progress * 100.0),
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
