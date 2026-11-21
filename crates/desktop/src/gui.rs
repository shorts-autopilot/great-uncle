//! The eframe window: five panes, a command line and ten faders.
//!
//! Drawing reads [`Snapshot`] only. Every button and drag goes through
//! [`Model`] as command text, so the window cannot mutate the show directly.

use std::time::{Duration, Instant};

use eframe::egui;
use openlight_app::{format_cue_number, App, PlaybackRow, Snapshot, TimecodeRowKind};
use openlight_desktop::model::step_percent;
use openlight_desktop::{Model, Pane};

use eframe::egui::{
    Align, Color32, FontId, Key, Layout, RichText, ScrollArea, Sense, TextEdit, TextStyle, Ui,
    ViewportCommand,
};

/// One repaint's data at most every 16 ms; a snapshot at least every 250 ms
/// even when nothing changed, so status counters stay live.
const SNAPSHOT_FLOOR: Duration = Duration::from_millis(250);
const FADER_INTERVAL: Duration = Duration::from_millis(33);

pub fn run(app: App) -> Result<(), String> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OpenLight")
            .with_inner_size([1280.0, 800.0])
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
}

impl Desktop {
    fn new(app: App, cc: &eframe::CreationContext<'_>) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        // The panes are tables: monospace by default keeps the columns honest.
        style
            .text_styles
            .insert(TextStyle::Body, FontId::monospace(13.0));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(13.0));
        cc.egui_ctx.set_style(style);

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
        }
    }

    fn act(&mut self, command: &str) {
        let _ = self.model.act(&self.app, command);
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
        let mut pane = None;
        let mut quit = false;
        ctx.input(|input| {
            for (key, candidate) in [
                (Key::F1, Pane::Sheet),
                (Key::F2, Pane::Playback),
                (Key::F3, Pane::Pools),
                (Key::F4, Pane::Timecode),
                (Key::F5, Pane::Status),
            ] {
                if input.key_pressed(key) {
                    pane = Some(candidate);
                }
            }
            if input.modifiers.command && input.key_pressed(Key::Q) {
                quit = true;
            }
            if input.key_pressed(Key::Escape) {
                self.model.input.clear();
            }
        });
        if let Some(pane) = pane {
            self.model.switch(pane);
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
        let mut action = None;
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("OpenLight");
                ui.label(format!("— {show}"));
                ui.separator();
                let label = if blackout { "BLACKOUT ON" } else { "BLACKOUT" };
                if ui.button(label).clicked() {
                    action = Some(if blackout {
                        "Blackout Off".to_string()
                    } else {
                        "Blackout".to_string()
                    });
                }
                if blind {
                    ui.colored_label(Color32::from_rgb(230, 190, 60), "BLIND");
                }
                if lock {
                    ui.colored_label(Color32::from_rgb(230, 90, 90), "LOCKED");
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
                });
            });
        });
        if let Some(action) = action {
            self.act(&action);
        }
    }

    fn pane_bar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("panes")
            .exact_width(126.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                for pane in Pane::ALL {
                    let selected = self.model.pane == pane;
                    if ui
                        .selectable_label(
                            selected,
                            RichText::new(format!("{}  {}", pane.function_key(), pane.title()))
                                .monospace(),
                        )
                        .clicked()
                    {
                        self.model.switch(pane);
                    }
                }
                ui.separator();
                ui.monospace(format!("{}", self.snapshot.desk.selection.len()));
                ui.label(RichText::new("selected").small());
            });
    }

    fn fader_bar(&mut self, ctx: &egui::Context) {
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
        egui::SidePanel::right("faders")
            .exact_width(330.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.monospace("FADERS");
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = 132.0;
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
                ui.add_space(6.0);
                let running = self.snapshot.playing.len();
                ui.monospace(format!("{running} playback(s) running"));
            });
        if pending.is_some() {
            self.pending_fader = pending;
        }
        if let Some(action) = flash_action {
            self.act(&action);
        }
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| match self.model.pane {
            Pane::Sheet => self.sheet(ui),
            Pane::Playback => self.playback(ui),
            Pane::Pools => self.pools(ui),
            Pane::Timecode => self.timecode(ui),
            Pane::Status => self.status(ui),
        });
    }

    fn sheet(&mut self, ui: &mut Ui) {
        ui.heading("Sheet");
        let rows = &self.snapshot.listings.sheet;
        let mut action = None;
        ui.horizontal(|ui| {
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
        ui.heading("Playback");
        let rows = &self.snapshot.listings.playback;
        if rows.is_empty() {
            ui.label("No cues stored.");
            return;
        }
        let selected = self.model.selected_sequence;
        let mut select = None;
        let mut transport = None;
        if let Some(sequence) = selected {
            ui.horizontal(|ui| {
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

    fn pools(&mut self, ui: &mut Ui) {
        ui.heading("Pools");
        let rows = &self.snapshot.listings.pools;
        if rows.is_empty() {
            ui.label("No palettes, groups, worlds, filters or effects stored.");
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
        ui.heading("Timecode");
        let rows = &self.snapshot.listings.timecode;
        if rows.is_empty() {
            ui.label("No timecode shows stored.");
            return;
        }
        ui.horizontal(|ui| {
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

    fn status(&mut self, ui: &mut Ui) {
        ui.heading("Status");
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
                ui.monospace("openlight>");
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
            let colour = if self.model.error {
                Color32::from_rgb(230, 90, 90)
            } else {
                Color32::from_gray(170)
            };
            ui.monospace(RichText::new(&self.model.message).color(colour));
            ui.label(
                RichText::new("F1–F5 panes · Enter run · drag a fader · Esc clear · Ctrl-Q quit")
                    .small(),
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
        self.pane_bar(ctx);
        self.fader_bar(ctx);
        self.central(ctx);
        self.flush_fader(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.stop();
    }
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
