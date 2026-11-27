//! The eframe window: the blank user-defined screen.
//!
//! Only the screen, its 12 × 8 snap grid and the windows placed on it are
//! drawn. Everything else — top bar, side rail, command line, window content
//! and the add-window picker — is gone; the surrounding UI is rebuilt one
//! piece at a time.

use eframe::egui;
use openlight_app::App;
use openlight_desktop::workspace::{GridRect, Window, WindowId, COLUMNS, ROWS};
use openlight_desktop::Model;

use eframe::egui::{
    Align, Align2, Color32, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, TextStyle,
    Ui, UiBuilder, Vec2,
};

const SCREEN: Color32 = Color32::from_rgb(9, 11, 15);
const WINDOW: Color32 = Color32::from_rgb(24, 28, 35);
const WINDOW_HEADER: Color32 = Color32::from_rgb(32, 37, 46);
const GRID_DOT: Color32 = Color32::from_rgb(38, 43, 52);
const ACCENT: Color32 = Color32::from_rgb(225, 165, 72);
const MUTED: Color32 = Color32::from_rgb(150, 157, 168);
const DANGER: Color32 = Color32::from_rgb(229, 94, 94);

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
    drag: Option<DragState>,
    resize: Option<ResizeState>,
}

impl Desktop {
    fn new(app: App, cc: &eframe::CreationContext<'_>) -> Self {
        apply_theme(&cc.egui_ctx);
        Self {
            app,
            model: Model::new(),
            drag: None,
            resize: None,
        }
    }

    fn workspace_panel(&mut self, ctx: &egui::Context) {
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
        });
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

fn paint_grid(ui: &Ui, screen: Rect, cell: Vec2) {
    for y in 0..ROWS {
        for x in 0..COLUMNS {
            let point = screen.min
                + Vec2::new((f32::from(x) + 0.5) * cell.x, (f32::from(y) + 0.5) * cell.y);
            ui.painter().circle_filled(point, 1.0, GRID_DOT);
        }
    }
}
