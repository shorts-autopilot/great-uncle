//! The eframe window: a user-defined screen of movable windows, a command
//! line and a dark console-style chrome.
//!
//! Drawing reads [`Snapshot`] only. Every action still becomes command text
//! and goes through [`Model`], so the window cannot mutate the show directly.

use std::time::{Duration, Instant};

use eframe::egui;
use openlight_app::{format_cue_number, App, PlaybackRow, Snapshot, TimecodeRowKind};
use openlight_desktop::model::step_percent;
use openlight_desktop::workspace::{
    GridRect, Window, WindowId, WindowKind, WindowTab, COLUMNS, ROWS,
};
use openlight_desktop::Model;

use eframe::egui::{
    Align, Align2, Button, Color32, FontId, Key, Label, Layout, Margin, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, TextEdit, TextStyle, Ui, UiBuilder, Vec2, ViewportCommand,
};

/// One repaint's data at most every 16 ms; a snapshot at least every 250 ms
/// even when nothing changed, so status counters stay live.
const SNAPSHOT_FLOOR: Duration = Duration::from_millis(250);
const FADER_INTERVAL: Duration = Duration::from_millis(33);

const SCREEN: Color32 = Color32::from_rgb(9, 11, 15);
const PANEL: Color32 = Color32::from_rgb(18, 21, 27);
const WINDOW: Color32 = Color32::from_rgb(24, 28, 35);
const WINDOW_HEADER: Color32 = Color32::from_rgb(32, 37, 46);
const GRID_DOT: Color32 = Color32::from_rgb(38, 43, 52);
const ACCENT: Color32 = Color32::from_rgb(225, 165, 72);
const MUTED: Color32 = Color32::from_rgb(150, 157, 168);
const DANGER: Color32 = Color32::from_rgb(229, 94, 94);
const OK: Color32 = Color32::from_rgb(111, 190, 139);

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
    snapshot: Snapshot,
    last_snapshot: Instant,
    last_fader_send: Instant,
    /// `(executor, 0..=1)`, latest value wins while a drag is throttled.
    pending_fader: Option<(u16, f64)>,
    faders: [f64; 9],
    grand: f64,
    /// Level the sheet's `At` button applies to the selection.
    at_level: f64,
    /// Attribute and value the sheet's `Def` button stores when both are set;
    /// an empty pair makes `Def` list the effective home instead.
    default_attribute: String,
    default_value: String,
    /// Position the timecode `Locate` and `Add` buttons send.
    timecode_at: String,
    /// Event text for the timecode `Add` button, e.g. `Sequence 1 Cue 1`.
    timecode_event: String,
    /// Code for the top bar's `UNLOCK` button when the desk is locked.
    unlock_code: String,
    focus_command: bool,
    show_add_picker: bool,
    picker_tab: WindowTab,
    /// A clicked empty grid cell, so the picker can place the new window there.
    pending_cell: Option<(u16, u16)>,
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
            unlock_code: String::new(),
            focus_command: true,
            show_add_picker: false,
            picker_tab: WindowTab::Common,
            pending_cell: None,
            drag: None,
            resize: None,
        }
    }

    fn act(&mut self, command: &str) {
        let _ = self.model.act(&self.app, command);
    }

    fn open_picker(&mut self, cell: Option<(u16, u16)>) {
        self.show_add_picker = true;
        self.pending_cell = cell;
    }

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
        let mut add = false;
        let mut quit = false;
        let mut escape = false;
        ctx.input(|input| {
            if input.key_pressed(Key::F1) {
                add = true;
            }
            if input.modifiers.command && input.key_pressed(Key::Q) {
                quit = true;
            }
            if input.key_pressed(Key::Escape) {
                escape = true;
            }
        });
        if add {
            if self.show_add_picker {
                self.show_add_picker = false;
                self.pending_cell = None;
            } else {
                self.open_picker(None);
            }
        }
        if escape {
            if self.show_add_picker {
                self.show_add_picker = false;
                self.pending_cell = None;
            } else {
                self.model.input.clear();
            }
        }
        if quit {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        let show = self.snapshot.show_name.clone();
        let blackout = self.snapshot.desk.blackout;
        let blind = self.snapshot.desk.blind;
        let lock = self.snapshot.desk.lock;
        let timecode = self.snapshot.desk.timecode.clone();
        let running = self.snapshot.playing.len();
        let mut action = None;
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("OPENLIGHT").strong().color(ACCENT));
                ui.separator();
                ui.label(RichText::new(show).monospace());
                ui.separator();
                let label = if blackout { "BLACKOUT ON" } else { "BLACKOUT" };
                if ui
                    .button(RichText::new(label).color(if blackout { DANGER } else { MUTED }))
                    .clicked()
                {
                    action = Some(if blackout {
                        "Blackout Off".to_string()
                    } else {
                        "Blackout".to_string()
                    });
                }
                if blind {
                    ui.colored_label(ACCENT, "BLIND");
                }
                if ui
                    .button(RichText::new("ADD WINDOW").color(ACCENT))
                    .clicked()
                {
                    self.open_picker(None);
                }
                if lock {
                    ui.colored_label(DANGER, "LOCKED");
                    ui.add(
                        TextEdit::singleline(&mut self.unlock_code)
                            .desired_width(60.0)
                            .hint_text("code"),
                    );
                    if ui.button("UNLOCK").clicked() {
                        let code = self.unlock_code.trim();
                        action = Some(if code.is_empty() {
                            "Unlock".to_string()
                        } else {
                            format!("Unlock {code}")
                        });
                    }
                } else if ui.button("LOCK").clicked() {
                    action = Some("Lock".to_string());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.monospace(timecode);
                    ui.separator();
                    ui.label(RichText::new(format!("{running} running")).color(MUTED));
                });
            });
        });
        if let Some(action) = action {
            self.act(&action);
        }
    }

    fn control_bar(&mut self, ctx: &egui::Context) {
        let page = self.snapshot.desk.page;
        let page_name = self.snapshot.desk.page_name.clone();
        let selection = self.snapshot.desk.selection.len();
        let running = self.snapshot.playing.len();
        egui::SidePanel::left("control")
            .exact_width(96.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                if ui
                    .add_sized(
                        [78.0, 32.0],
                        Button::new(RichText::new("+  ADD").color(ACCENT)),
                    )
                    .on_hover_text("Add a window (F1)")
                    .clicked()
                {
                    self.open_picker(None);
                }
                if ui
                    .add_sized([78.0, 28.0], Button::new("CLEAR SCREEN"))
                    .on_hover_text("Remove every window without touching the show")
                    .clicked()
                {
                    self.model.workspace.clear();
                    self.drag = None;
                    self.resize = None;
                }
                if ui
                    .add_sized([78.0, 28.0], Button::new("RESET"))
                    .on_hover_text("Clear the screen and close the picker")
                    .clicked()
                {
                    self.model.workspace.clear();
                    self.show_add_picker = false;
                    self.pending_cell = None;
                    self.drag = None;
                    self.resize = None;
                }
                ui.separator();
                ui.label(RichText::new("PAGE").small().color(MUTED));
                ui.label(RichText::new(format!("{page}")).heading().color(ACCENT));
                ui.label(RichText::new(page_name).small());
                ui.separator();
                ui.label(RichText::new("SELECTED").small().color(MUTED));
                ui.label(RichText::new(selection.to_string()).heading());
                ui.separator();
                ui.label(RichText::new("RUNNING").small().color(MUTED));
                ui.label(RichText::new(running.to_string()).heading());
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("ADD  F1\nCLEAR  —\nRESET  —")
                            .small()
                            .color(MUTED),
                    );
                });
            });
    }

    fn workspace_panel(&mut self, ctx: &egui::Context) {
        let mut clicked_cell = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            let screen = ui.max_rect();
            let cell = Vec2::new(
                (screen.width() / f32::from(COLUMNS)).max(1.0),
                (screen.height() / f32::from(ROWS)).max(1.0),
            );
            ui.painter().rect_filled(screen, 0.0, SCREEN);
            paint_grid(ui, screen, cell);

            if self.model.workspace.is_empty() {
                ui.painter().text(
                    screen.center() - Vec2::new(0.0, 26.0),
                    Align2::CENTER_CENTER,
                    "SCREEN 1 IS EMPTY",
                    FontId::monospace(22.0),
                    MUTED,
                );
                ui.painter().text(
                    screen.center() + Vec2::new(0.0, 2.0),
                    Align2::CENTER_CENTER,
                    "Add windows to build a workspace",
                    FontId::monospace(13.0),
                    Color32::from_gray(110),
                );
                let button_rect = Rect::from_center_size(
                    screen.center() + Vec2::new(0.0, 44.0),
                    Vec2::new(190.0, 38.0),
                );
                if ui
                    .put(
                        button_rect,
                        Button::new(RichText::new("+  ADD WINDOW").color(ACCENT)),
                    )
                    .on_hover_text("Open the Add Window picker")
                    .clicked()
                {
                    self.open_picker(None);
                }
            }

            // Read the click directly instead of registering a full-screen
            // interactive widget: a background widget would compete with the
            // windows for hit testing. A click inside a window is rejected by
            // `window_at`; the picker itself owns clicks while it is open.
            let clicked = ui.input(|input| input.pointer.primary_clicked());
            if clicked && !self.show_add_picker {
                if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                    if screen.contains(pointer) {
                        if let Some(cell) = grid_cell(screen, cell, pointer) {
                            if self.model.workspace.window_at(cell.0, cell.1).is_none() {
                                clicked_cell = Some(cell);
                            }
                        }
                    }
                }
            }

            let windows = self.model.workspace.windows().to_vec();
            for window in windows {
                self.draw_window(ui, &window, screen, cell);
            }
            self.apply_layout_gestures(ctx, cell);
        });
        if let Some(cell) = clicked_cell {
            self.open_picker(Some(cell));
        }
        self.add_picker(ctx);
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

    fn add_picker(&mut self, ctx: &egui::Context) {
        if !self.show_add_picker {
            return;
        }
        let mut open = true;
        let mut close_picker = false;
        let mut selected_tab = self.picker_tab;
        let mut add = None;
        egui::Window::new("Add Window")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Choose a window for this screen. It snaps to the grid.")
                        .color(MUTED),
                );
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for tab in WindowTab::ALL {
                        if ui
                            .selectable_label(selected_tab == tab, tab.title())
                            .clicked()
                        {
                            selected_tab = tab;
                        }
                    }
                });
                ui.separator();
                let kinds = selected_tab.kinds();
                egui::Grid::new("add-window-grid")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        for (index, kind) in kinds.iter().enumerate() {
                            let (width, height) = kind.default_size();
                            let label = format!("{}   {}×{}", kind.title(), width, height);
                            if ui
                                .add_sized([252.0, 34.0], Button::new(label))
                                .on_hover_text(kind.description())
                                .clicked()
                            {
                                add = Some(*kind);
                            }
                            if index % 2 == 1 {
                                ui.end_row();
                            }
                        }
                        if kinds.len() % 2 == 1 {
                            ui.end_row();
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("The screen is session state; it starts blank.")
                            .small()
                            .color(MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close_picker = true;
                        }
                    });
                });
            });
        self.picker_tab = selected_tab;
        if close_picker {
            open = false;
        }
        if let Some(kind) = add {
            if self.model.add_window_at(kind, self.pending_cell).is_none() {
                self.model.message =
                    "No free grid space for that window. Close or resize one first.".into();
                self.model.error = false;
            } else {
                self.show_add_picker = false;
                self.pending_cell = None;
            }
        }
        if !open {
            self.show_add_picker = false;
            self.pending_cell = None;
        }
    }

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
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(16.0, 14.0),
                                        Sense::hover(),
                                    );
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
                                action = Some(openlight_desktop::model::pool_command(
                                    &row.kind,
                                    row.number,
                                    &row.detail,
                                    row.active,
                                ));
                            }
                            if ui.small_button("Del").clicked() {
                                action = Some(openlight_desktop::model::delete_command(
                                    &row.kind,
                                    row.number,
                                    &row.detail,
                                ));
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
                        let state = if playback.done {
                            "complete".to_string()
                        } else if playback.paused {
                            "paused".to_string()
                        } else {
                            "running".to_string()
                        };
                        ui.label(if playback.done || playback.paused {
                            RichText::new(state).color(MUTED)
                        } else {
                            RichText::new(state).color(OK)
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

    fn command_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("command").show(ctx, |ui| {
            if let Some(echo) = self.model.last_echo() {
                ui.monospace(RichText::new(format!("> {echo}")).color(Color32::from_gray(140)));
            }
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
            ui.label(
                RichText::new("F1 add window · click empty screen · drag title / corner · Enter run · Ctrl-Q quit")
                    .small()
                    .color(MUTED),
            );
        });
    }
}

impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.keys(ctx);
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

        self.top_bar(ctx);
        self.command_bar(ctx);
        self.control_bar(ctx);
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
    visuals.panel_fill = PANEL;
    visuals.window_fill = WINDOW;
    visuals.extreme_bg_color = SCREEN;
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

fn paint_grid(ui: &Ui, screen: Rect, cell: Vec2) {
    for y in 0..ROWS {
        for x in 0..COLUMNS {
            let point = screen.min
                + Vec2::new((f32::from(x) + 0.5) * cell.x, (f32::from(y) + 0.5) * cell.y);
            ui.painter().circle_filled(point, 1.0, GRID_DOT);
        }
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

/// A 5% keyboard nudge when a fader has focus.
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
