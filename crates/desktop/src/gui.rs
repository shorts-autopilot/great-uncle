//! The eframe window: a user-defined screen of movable windows.
//!
//! The screen, its 12 × 8 dotted cell grid and any placed window are drawn. A
//! click on empty canvas opens the Add Window picker at that point; choosing a
//! catalogue tile places that window at the clicked cell and closes the picker.
//!
//! Three pieces share the work. This file owns the canvas, the window frame and
//! the eframe loop; `panes` draws what each window kind shows; `picker` draws
//! the catalogue. Every control still becomes command text through [`Model`],
//! so no pane can mutate the show directly.

mod panes;
mod picker;

use std::time::{Duration, Instant};

use eframe::egui;
use openlight_app::{App, Snapshot};
use openlight_desktop::workspace::{GridRect, Window, WindowId, WindowTab, COLUMNS, ROWS};
use openlight_desktop::Model;

use eframe::egui::{
    Align, Align2, Color32, FontId, Key, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke,
    TextStyle, Ui, UiBuilder, Vec2,
};

const SCREEN: Color32 = Color32::from_rgb(9, 11, 15);
const WINDOW: Color32 = Color32::from_rgb(24, 28, 35);
const WINDOW_HEADER: Color32 = Color32::from_rgb(32, 37, 46);
/// The pane body sits a shade darker than the frame, so the area a window
/// fills reads at a glance without competing with its content.
const PANE_FILL: Color32 = Color32::from_rgb(20, 24, 30);
const GRID_LINE: Color32 = Color32::from_rgb(38, 43, 52);
/// One thin line on the grid cells a window claims, so its footprint is
/// visible even when its pane is empty.
const WINDOW_OUTLINE: Color32 = Color32::from_rgb(74, 84, 98);
const ACCENT: Color32 = Color32::from_rgb(225, 165, 72);
const MUTED: Color32 = Color32::from_rgb(150, 157, 168);
const DANGER: Color32 = Color32::from_rgb(229, 94, 94);
const OK: Color32 = Color32::from_rgb(111, 190, 139);

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
    /// Focus a command prompt on the next frame.
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
        let mut desktop = Self {
            app,
            model: Model::new(),
            snapshot,
            last_snapshot: Instant::now(),
            last_fader_send: Instant::now(),
            pending_fader: None,
            faders: [100.0; 9],
            grand: 100.0,
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
        };
        desktop.sync_faders();
        desktop
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
                let cell = cell_size(screen);
                ui.painter().rect_filled(screen, 0.0, SCREEN);
                paint_grid(ui, screen, cell);

                if self.model.workspace.is_empty() {
                    paint_empty_screen(ui, screen);
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

    fn draw_window(&mut self, ui: &mut Ui, window: &Window, screen: Rect, cell: Vec2) {
        let footprint = Rect::from_min_size(
            screen.min
                + Vec2::new(
                    f32::from(window.rect.x) * cell.x,
                    f32::from(window.rect.y) * cell.y,
                ),
            Vec2::new(
                f32::from(window.rect.width) * cell.x,
                f32::from(window.rect.height) * cell.y,
            ),
        );
        let window_rect = footprint.shrink(2.0);
        if window_rect.width() < 8.0 || window_rect.height() < 8.0 {
            return;
        }

        // The outline shows exactly which grid cells the window occupies; the
        // frame is inset inside it, so the claimed space stays readable.
        ui.painter().rect_stroke(
            footprint.shrink(1.0),
            egui::CornerRadius::same(3),
            Stroke::new(1.0_f32, WINDOW_OUTLINE),
            egui::StrokeKind::Inside,
        );

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
                // A faint body fill makes the used area of the window obvious
                // on the dark canvas.
                let body = ui.available_rect_before_wrap();
                ui.painter().rect_filled(body, 2, PANE_FILL);
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
}

impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.keys(ctx);
        // Rebuild when the show changed, and at least every 250 ms so the DMX
        // sheet and status counters stay live.
        let dirty = self.model.take_dirty();
        if dirty
            || self.app.revision() != self.snapshot.revision
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

/// One grid cell's size in pixels.
fn cell_size(screen: Rect) -> Vec2 {
    Vec2::new(
        (screen.width() / f32::from(COLUMNS)).max(1.0),
        (screen.height() / f32::from(ROWS)).max(1.0),
    )
}

fn paint_empty_screen(ui: &Ui, screen: Rect) {
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
