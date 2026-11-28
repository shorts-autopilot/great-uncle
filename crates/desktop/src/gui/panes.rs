//! The pane inside each window kind.
//!
//! Every pane reads [`Snapshot`] and never touches the engine: a control turns
//! a click into the same command text a typist would send and hands it to
//! [`Model`]. That is what keeps the window from becoming a second engine.
//!
//! [`Snapshot`]: openlight_app::Snapshot
//! [`Model`]: openlight_desktop::Model

use eframe::egui;
use eframe::egui::{Color32, Key, Label, RichText, ScrollArea, Sense, TextEdit, TextStyle, Ui};
use openlight_app::{format_cue_number, PlaybackRow, TimecodeRowKind};
use openlight_desktop::model::{delete_command, pool_command, step_percent};
use openlight_desktop::workspace::WindowKind::{
    self, CommandLine, CommandLineHistory, DmxSheet, Effects, Filters, FixtureSheet, Groups,
    Masters, Patch, Playbacks, Pools, Presets, RunningPlaybacks, SequenceSheet, Status, Timecode,
    Worlds,
};

use super::{Desktop, ACCENT, DANGER, MUTED, OK};

impl Desktop {
    /// Draw the pane for one window kind from the current snapshot.
    pub(super) fn window_body(&mut self, kind: WindowKind, ui: &mut Ui) {
        match kind {
            FixtureSheet => self.sheet(ui),
            SequenceSheet => self.playback(ui),
            Playbacks => self.playbacks(ui),
            Pools => self.pools(ui, None),
            Presets => self.pools(ui, Some("Palette")),
            Groups => self.pools(ui, Some("Group")),
            Worlds => self.pools(ui, Some("World")),
            Filters => self.pools(ui, Some("Filter")),
            Effects => self.pools(ui, Some("Effect")),
            Timecode => self.timecode(ui),
            CommandLine => self.command_line(ui),
            CommandLineHistory => self.command_history(ui),
            DmxSheet => self.dmx_sheet(ui),
            Masters => self.masters(ui),
            RunningPlaybacks => self.running_playbacks(ui),
            Patch => self.patch(ui),
            Status => self.status(ui),
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
        if !self
            .snapshot
            .listings
            .pools
            .iter()
            .any(|row| pool_matches(&row.kind, filter))
        {
            ui.label(match filter {
                Some(kind) => format!("No {kind} entries stored."),
                None => "No palettes, groups, worlds, filters or effects stored.".to_string(),
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
                        for row in self
                            .snapshot
                            .listings
                            .pools
                            .iter()
                            .filter(|row| pool_matches(&row.kind, filter))
                        {
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
        ui.monospace(format!(
            "Blackout  {}",
            yes_no(self.snapshot.desk.blackout)
        ));
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

    /// A command prompt and the last result, for the Command Line window.
    fn command_line(&mut self, ui: &mut Ui) {
        self.command_prompt(ui);
        if let Some(echo) = self.model.last_echo() {
            ui.monospace(RichText::new(format!("> {echo}")).color(Color32::from_gray(140)));
        }
        let colour = if self.model.error { DANGER } else { MUTED };
        ui.monospace(RichText::new(&self.model.message).color(colour));
        ui.label(
            RichText::new("Enter runs · click a history line to recall it")
                .small()
                .color(MUTED),
        );
    }

    fn command_history(&mut self, ui: &mut Ui) {
        self.command_prompt(ui);
        let colour = if self.model.error { DANGER } else { MUTED };
        ui.monospace(RichText::new(&self.model.message).color(colour));
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

    /// The one prompt both command windows use: Enter submits through
    /// `Model::submit`, so its echo and history are the same as the terminal's.
    fn command_prompt(&mut self, ui: &mut Ui) {
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
    }
}

fn pool_matches(kind: &str, filter: Option<&str>) -> bool {
    filter.is_none_or(|wanted| kind == wanted)
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
