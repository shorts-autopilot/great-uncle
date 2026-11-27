//! The eframe window: the blank user-defined screen.
//!
//! The screen, its 12 × 8 dotted cell grid and any placed window are drawn. A
//! click on empty canvas opens the Add Window picker at that point; choosing a
//! catalogue tile places that window at the clicked cell and closes the
//! picker. The rest of the chrome is rebuilt one piece at a time.

use eframe::egui;
use openlight_app::App;
use openlight_desktop::workspace::{
    GridRect, Window, WindowId, WindowKind, WindowTab, COLUMNS, ROWS,
};
use openlight_desktop::Model;

use eframe::egui::{
    Align, Align2, Color32, FontId, Key, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke,
    TextStyle, Ui, UiBuilder, Vec2,
};

const SCREEN: Color32 = Color32::from_rgb(9, 11, 15);
const WINDOW: Color32 = Color32::from_rgb(24, 28, 35);
const WINDOW_HEADER: Color32 = Color32::from_rgb(32, 37, 46);
const GRID_LINE: Color32 = Color32::from_rgb(38, 43, 52);
const ACCENT: Color32 = Color32::from_rgb(225, 165, 72);
const MUTED: Color32 = Color32::from_rgb(150, 157, 168);
const DANGER: Color32 = Color32::from_rgb(229, 94, 94);

const PICKER_WIDTH: f32 = 780.0;
const PICKER_HEIGHT: f32 = 300.0;
const PICKER_COLUMNS: usize = 4;
const TILE_GAP: f32 = 10.0;
const TILE_HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 32.0;

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
        Self {
            app,
            model: Model::new(),
            show_add_picker: false,
            picker_pos: Pos2::ZERO,
            picker_cell: None,
            picker_tab: WindowTab::Common,
            drag: None,
            resize: None,
        }
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
                for window in windows {
                    self.draw_window(ui, &window, screen, cell);
                }
                self.apply_layout_gestures(ctx, cell);

                // A click on empty canvas opens the Add Window picker. A click
                // inside a placed window does not, and the picker owns its own
                // clicks while it is open.
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
                // No window content yet: hold the frame at its placed size.
                ui.allocate_space(ui.available_size());

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
        self.workspace_panel(ctx);
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
