use crate::{
    audio::{self, AudioEvent, Capture, Runtime},
    delivery,
    journal::Journal,
    model::{Destination, Event},
    shortcuts::{Action, Registry, Shortcuts},
};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone, Utc};
use eframe::egui::{self, Color32, RichText};
use egui_native_look as look;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration as StdDuration,
};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub folder: PathBuf,
    pub mic: bool,
    pub output: bool,
    pub microphone: String,
    pub speakers: String,
    #[serde(default)]
    pub model: audio::ModelChoice,
    #[serde(default)]
    pub keep_audio: bool,
    #[serde(default)]
    pub shortcuts: Shortcuts,
    #[serde(default = "yes")]
    pub tray_countdown: bool,
    #[serde(default = "yes")]
    pub tray_levels: bool,
}
fn yes() -> bool {
    true
}
impl Settings {
    fn path() -> Result<PathBuf> {
        Ok(PathBuf::from(std::env::var("HOME")?)
            .join("Library/Application Support/team-up/tray-settings.json"))
    }
    fn load() -> Result<Self> {
        let path = Self::path()?;
        if path.exists() {
            return Ok(serde_json::from_slice(&std::fs::read(path)?)?);
        }
        Ok(Self {
            folder: PathBuf::from(std::env::var("HOME")?).join("Documents/Team-up"),
            mic: true,
            output: false,
            microphone: String::new(),
            speakers: String::new(),
            model: audio::ModelChoice::default(),
            keep_audio: false,
            shortcuts: Shortcuts::default(),
            tray_countdown: true,
            tray_levels: true,
        })
    }
    fn save(&self) -> Result<()> {
        let path = Self::path()?;
        let parent = path.parent().context("Invalid settings path")?;
        std::fs::create_dir_all(parent)?;
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}
pub enum Work {
    Chats(Result<Vec<Destination>, String>),
    Created(Result<Destination, String>),
    Delivered(uuid::Uuid, std::result::Result<(), delivery::SendFailure>),
}
pub struct Panel {
    pub journal: Journal,
    pub settings: Settings,
    pub recording_since: Option<DateTime<Utc>>,
    pub status: String,
    pub range: String,
    range_dirty: bool,
    pub choosing: bool,
    pub chats: Vec<Destination>,
    pub new_title: String,
    pub busy: bool,
    flush_pending: Option<(u64, std::collections::BTreeSet<String>)>,
    runtime: Option<Runtime>,
    capture: Option<Capture>,
    audio_tx: Sender<AudioEvent>,
    audio_rx: Receiver<AudioEvent>,
    work_tx: Sender<Work>,
    work_rx: Receiver<Work>,
    tray: Option<TrayIcon>,
    tray_state: String,
    microphones: Vec<String>,
    speakers: Vec<String>,
    pub test_mode: bool,
    quitting: bool,
    pub settings_open: bool,
    pub microphone_muted: bool,
    pub listening: Option<Action>,
    hotkeys: Option<Registry>,
    hotkey_error: bool,
    meters: (f32, f32),
    panel_visible: std::sync::Arc<std::sync::atomic::AtomicBool>,
    window_height: f32,
}
impl Panel {
    fn save_settings(&self) -> Result<()> {
        if self.test_mode {
            Ok(())
        } else {
            self.settings.save()
        }
    }
    pub fn open() -> Result<Self> {
        let settings = Settings::load()?;
        let journal = Journal::open(&settings.folder)?;
        Ok(Self::with_journal(journal, settings, false))
    }
    pub fn with_journal(journal: Journal, settings: Settings, test_mode: bool) -> Self {
        let (audio_tx, audio_rx) = mpsc::channel();
        let (work_tx, work_rx) = mpsc::channel();
        let range = local_time(journal.model.range_start);
        let status = if journal.model.uncertain {
            "Previous delivery is uncertain. Check the receiving chat before resolving it.".into()
        } else {
            String::new()
        };
        Self {
            journal,
            settings,
            recording_since: None,
            status,
            range,
            range_dirty: false,
            choosing: false,
            chats: vec![],
            new_title: String::new(),
            busy: false,
            flush_pending: None,
            runtime: None,
            capture: None,
            audio_tx,
            audio_rx,
            work_tx,
            work_rx,
            tray: None,
            tray_state: String::new(),
            microphones: if test_mode {
                vec![]
            } else {
                audio::devices(false)
            },
            speakers: if test_mode {
                vec![]
            } else {
                audio::devices(true)
            },
            test_mode,
            quitting: false,
            settings_open: false,
            microphone_muted: false,
            listening: None,
            hotkeys: None,
            hotkey_error: false,
            meters: (0.0, 0.0),
            panel_visible: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            window_height: 0.0,
        }
    }
    fn event(&mut self, event: Event) -> bool {
        match self.journal.append(event) {
            Ok(()) => true,
            Err(error) => {
                self.status = format!("Cannot save journal: {error:#}");
                self.stop();
                false
            }
        }
    }
    fn stop(&mut self) {
        if let Some(capture) = &mut self.capture {
            capture.stop();
        }
        self.recording_since = None;
    }
    pub fn is_draining(&self) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|capture| !capture.finished())
            && self.recording_since.is_none()
    }
    pub fn toggle_recording(&mut self) {
        if self.recording_since.is_some() {
            self.stop();
            self.event(Event::Recording {
                at: Utc::now(),
                active: false,
            });
            return;
        }
        if self.capture.as_ref().is_some_and(|c| !c.finished()) {
            self.status = "Finishing the previous audio stream…".into();
            return;
        }
        if !self.settings.mic && !self.settings.output {
            self.status = "Select a microphone or system audio.".into();
            return;
        }
        if !self.test_mode {
            if self.runtime.is_none() && self.settings.model == audio::ModelChoice::Nemotron {
                match Runtime::start() {
                    Ok(runtime) => self.runtime = Some(runtime),
                    Err(error) => {
                        self.status = error.to_string();
                        return;
                    }
                }
            }
            let port = self
                .runtime
                .as_ref()
                .map(|runtime| runtime.port)
                .unwrap_or(0);
            let mut selections = vec![];
            if self.settings.mic {
                selections.push(("microphone".into(), false, self.settings.microphone.clone()));
            }
            if self.settings.output {
                selections.push(("system audio".into(), true, self.settings.speakers.clone()));
            }
            self.capture = Some(Capture::start(
                port,
                selections,
                audio::CaptureOptions {
                    model: self.settings.model,
                    audio_directory: Some(self.settings.folder.join("audio")),
                    retain_audio: self.settings.keep_audio,
                    microphone_muted: self.microphone_muted,
                },
                self.audio_tx.clone(),
            ));
        }
        if let Some(capture) = &self.capture {
            capture
                .muted
                .store(self.microphone_muted, std::sync::atomic::Ordering::Relaxed);
        }
        let now = Utc::now();
        if self.event(Event::Recording {
            at: now,
            active: true,
        }) {
            self.recording_since = Some(now);
            self.status = if self.test_mode {
                String::new()
            } else {
                "Starting local speech recognition…".into()
            };
        }
    }
    pub fn choose(&mut self, destination: Destination) {
        if self.journal.model.pending.is_some() {
            self.status = "Resolve the pending send before changing chats.".into();
            return;
        }
        if self.event(Event::Conversation {
            at: Utc::now(),
            destination: Some(destination),
        }) {
            self.range = local_time(self.journal.model.range_start);
            if let Some(capture) = &self.capture {
                capture.flush();
            }
            self.choosing = false;
            self.status.clear();
        }
    }
    fn load_chats(&mut self) {
        self.journal.model.auto = false;
        self.journal.model.next_send = None;
        self.choosing = true;
        self.busy = true;
        if self.test_mode {
            self.busy = false;
            return;
        }
        let tx = self.work_tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Work::Chats(delivery::list().map_err(|e| e.to_string())));
        });
    }
    pub fn send(&mut self) {
        if self.busy {
            return;
        }
        if self.journal.model.destination.is_none() {
            self.status = "Choose a chat first.".into();
            return;
        }
        if self.journal.model.uncertain {
            self.status = "Resolve the uncertain delivery first.".into();
            return;
        }
        if !self.event(Event::SendClicked { at: Utc::now() }) {
            return;
        }
        if let Some(capture) = &self.capture
            && !capture.finished()
        {
            let mut waiting = std::collections::BTreeSet::new();
            if self.settings.mic {
                waiting.insert("microphone".into());
            }
            if self.settings.output {
                waiting.insert("system audio".into());
            }
            self.flush_pending = Some((capture.flush(), waiting));
            self.busy = true;
            self.status = "Finalizing speech…".into();
            return;
        }
        self.dispatch();
    }
    fn dispatch(&mut self) {
        self.busy = false;
        let Some(batch) = self.journal.model.batch(Utc::now()) else {
            self.status = "No finalized speech in this range.".into();
            self.journal.model.rearm(Utc::now());
            return;
        };
        if !self.event(Event::SendRequested {
            batch: batch.clone(),
        }) {
            return;
        }
        self.busy = true;
        if self.test_mode {
            return;
        }
        let tx = self.work_tx.clone();
        thread::spawn(move || {
            let result = delivery::send(&batch);
            let _ = tx.send(Work::Delivered(batch.id, result));
        });
    }
    pub fn tick(&mut self) {
        while let Ok(event) = self.audio_rx.try_recv() {
            match event {
                AudioEvent::Transcript(segment) => {
                    self.ingest(segment);
                }
                AudioEvent::Flushed { generation, source } => {
                    if let Some((pending, waiting)) = &mut self.flush_pending
                        && *pending == generation
                    {
                        waiting.remove(&source);
                    }
                }
                AudioEvent::Error(error) => {
                    self.flush_pending = None;
                    self.busy = false;
                    self.stop();
                    self.event(Event::Recording {
                        at: Utc::now(),
                        active: false,
                    });
                    self.status = error;
                }
                AudioEvent::Ready => self.status.clear(),
            }
        }
        if self.flush_pending.as_ref().is_some_and(|(_, waiting)| {
            waiting.is_empty() || self.capture.as_ref().is_some_and(Capture::finished)
        }) {
            self.flush_pending = None;
            self.dispatch();
        }
        while let Ok(work) = self.work_rx.try_recv() {
            self.busy = false;
            match work {
                Work::Chats(result) => match result {
                    Ok(chats) => {
                        self.chats = chats;
                        self.status.clear();
                    }
                    Err(error) => self.status = error,
                },
                Work::Created(result) => match result {
                    Ok(chat) => self.choose(chat),
                    Err(error) => self.status = error,
                },
                Work::Delivered(id, result) => match result {
                    Ok(()) => {
                        if self.event(Event::SendAccepted { id, at: Utc::now() }) {
                            self.range = local_time(self.journal.model.range_start);
                            self.status = "Delivered to Codex.".into();
                        }
                    }
                    Err(delivery::SendFailure::Rejected(message)) => {
                        self.event(Event::SendFailed {
                            id,
                            at: Utc::now(),
                            message: message.clone(),
                        });
                        self.status = format!("Not sent: {message}");
                    }
                    Err(delivery::SendFailure::Unconfirmed(message)) => {
                        self.event(Event::SendUncertain {
                            id,
                            at: Utc::now(),
                            message: message.clone(),
                        });
                        self.status = format!("Delivery not confirmed: {message}");
                    }
                },
            }
        }
        if self.journal.model.auto
            && !self.choosing
            && !self.busy
            && self
                .journal
                .model
                .next_send
                .is_some_and(|at| at <= Utc::now())
        {
            self.send();
        }
    }
    pub fn ingest(&mut self, segment: crate::model::Segment) {
        self.event(Event::Transcript { segment });
        if !self.range_dirty {
            self.range = local_time(self.journal.model.range_start);
        }
    }
    pub fn change_folder(&mut self, folder: PathBuf) -> Result<()> {
        if folder == self.journal.directory {
            return Ok(());
        }
        if self.recording_since.is_some()
            || self.is_draining()
            || self.journal.model.pending.is_some()
        {
            anyhow::bail!("Stop recording and finish pending sends before moving storage.");
        }
        std::fs::create_dir_all(&folder)?;
        let entries: Vec<_> =
            std::fs::read_dir(&self.journal.directory)?.collect::<std::io::Result<Vec<_>>>()?;
        for entry in &entries {
            if entry.path().extension().is_some_and(|e| e == "jsonl")
                && folder.join(entry.file_name()).exists()
            {
                anyhow::bail!("The selected folder already contains a day log with the same name.");
            }
        }
        for entry in entries {
            if entry.path().extension().is_some_and(|e| e == "jsonl") {
                let destination = folder.join(entry.file_name());
                std::fs::copy(entry.path(), &destination)?;
                std::fs::File::open(destination)?.sync_all()?;
            }
        }
        let audio = self.journal.directory.join("audio");
        if audio.is_dir() {
            let target = folder.join("audio");
            std::fs::create_dir_all(&target)?;
            for entry in std::fs::read_dir(audio)? {
                let entry = entry?;
                if entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "wav")
                {
                    let destination = target.join(entry.file_name());
                    if destination.exists() {
                        anyhow::bail!("The destination already contains this audio recording.");
                    }
                    std::fs::copy(entry.path(), &destination)?;
                    std::fs::File::open(destination)?.sync_all()?;
                }
            }
        }
        self.settings.folder = folder.clone();
        self.save_settings()?;
        self.journal.directory = folder;
        Ok(())
    }
    pub fn contents(&mut self, ui: &mut egui::Ui) {
        look::follow_system(ui.ctx());
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 7.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("team-up").size(15.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let response = ui.add(look::IconButton::new("").accessible_label("Settings"));
                let rect = response.rect;
                let painter = ui.painter();
                let center = rect.center();
                painter.circle_stroke(
                    center,
                    6.0,
                    egui::Stroke::new(1.5, ui.visuals().text_color()),
                );
                painter.circle_stroke(
                    center,
                    2.0,
                    egui::Stroke::new(1.5, ui.visuals().text_color()),
                );
                for i in 0..8 {
                    let angle = i as f32 * std::f32::consts::TAU / 8.0;
                    let direction = egui::vec2(angle.cos(), angle.sin());
                    painter.line_segment(
                        [center + direction * 6.0, center + direction * 9.0],
                        egui::Stroke::new(2.0, ui.visuals().text_color()),
                    );
                }
                if response.clicked() {
                    self.settings_open = !self.settings_open;
                }
            });
        });
        if self.settings_open {
            self.settings_contents(ui);
            return;
        }
        ui.add_space(2.0);
        let label = match self.recording_since {
            Some(since) => format!(
                "Recording\nSince {}",
                since.with_timezone(&Local).format("%H:%M")
            ),
            None => "Start recording".into(),
        };
        let record = ui
            .scope(|ui| {
                ui.spacing_mut().interact_size.y = 58.0;
                ui.add(
                    look::Button::new(label)
                        .width(ui.available_width())
                        .selected(self.recording_since.is_some()),
                )
            })
            .inner;
        if self.recording_since.is_some() {
            ui.painter().circle_filled(
                record.rect.left_center() + egui::vec2(16.0, 0.0),
                4.0,
                Color32::from_rgb(232, 66, 74),
            );
        }
        if record.clicked() {
            self.toggle_recording();
        }
        ui.add_space(2.0);
        if ui
            .add_enabled(
                self.settings.mic,
                look::Button::new(if self.microphone_muted {
                    "Unmute microphone"
                } else {
                    "Mute microphone"
                }),
            )
            .clicked()
        {
            self.action(Action::Mute);
        }
        if ui
            .add(look::Checkbox::new(
                &mut self.settings.keep_audio,
                "Keep recorded audio",
            ))
            .changed()
        {
            if let Some(capture) = &self.capture {
                capture.retaining.store(
                    self.settings.keep_audio,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            if let Err(error) = self.save_settings() {
                self.status = error.to_string();
            }
        }
        ui.separator();
        ui.label(RichText::new("Send to").strong());
        if self.choosing {
            self.picker(ui);
        } else {
            let target = self
                .journal
                .model
                .destination
                .as_ref()
                .map(|d| format!("{}\n{}", d.title, d.provider))
                .unwrap_or_else(|| "Choose a chat…".into());
            let destination = ui
                .scope(|ui| {
                    ui.spacing_mut().interact_size.y = 44.0;
                    ui.add_enabled(
                        !self.busy && !self.journal.model.uncertain,
                        look::Button::new(target).width(ui.available_width()),
                    )
                })
                .inner;
            if destination.clicked() {
                self.load_chats();
            }
            if self.journal.model.destination.is_some() {
                ui.horizontal(|ui| {
                    if ui.add(look::Button::new("Open in Codex")).clicked()
                        && let Some(destination) = &self.journal.model.destination
                        && let Err(error) = delivery::open_chat(destination)
                    {
                        self.status = error.to_string();
                    }

                    if ui
                        .add_enabled(
                            !self.busy && !self.journal.model.uncertain,
                            look::Button::new("Switch chat"),
                        )
                        .clicked()
                    {
                        self.load_chats();
                    }
                    if ui
                        .add_enabled(
                            !self.busy && !self.journal.model.uncertain,
                            look::Button::new("New chat"),
                        )
                        .clicked()
                    {
                        self.load_chats();
                        self.new_title = "New conversation".into();
                    }
                });
            }
        }
        ui.add_space(2.0);
        ui.label(RichText::new("Send range").strong());
        ui.horizontal(|ui| {
            let response = ui.add_sized(
                [ui.available_width() * 0.61, 28.0],
                look::TextField::new(&mut self.range)
                    .id_salt("send-range")
                    .hint("YYYY-MM-DD HH:MM")
                    .accessible_label("Send range start"),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Send range start")
            });
            self.range_dirty |= response.changed();
            if response.lost_focus() && self.range_dirty {
                self.range_dirty = false;
                let parsed = NaiveDateTime::parse_from_str(&self.range, "%Y-%m-%d %H:%M")
                    .ok()
                    .and_then(|t| Local.from_local_datetime(&t).single());
                match parsed {
                    Some(at) if at <= Local::now() => {
                        self.event(Event::Range {
                            at: at.with_timezone(&Utc),
                        });
                        self.range = local_time(self.journal.model.range_start);
                    }
                    _ => self.status = "Use an unambiguous local date and time before now.".into(),
                }
            }
            let (arrow, _) = ui.allocate_exact_size(egui::vec2(14.0, 20.0), egui::Sense::hover());
            ui.painter().arrow(
                arrow.left_center(),
                egui::vec2(12.0, 0.0),
                egui::Stroke::new(1.0, Color32::GRAY),
            );
            let mut now = "Now".to_owned();
            ui.add_enabled(
                false,
                look::TextField::new(&mut now)
                    .width(ui.available_width())
                    .accessible_label("Send range end: now"),
            )
            .widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, false, "Send range end: now")
            });
        });
        ui.add_space(4.0);
        let enabled =
            self.journal.model.destination.is_some() && !self.busy && !self.journal.model.uncertain;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let toggle = ui.add(
                    look::Switch::new(&mut self.journal.model.auto)
                        .accessible_label("Auto-send every"),
                );
                let label = ui.label("Auto-send every");
                let toggle = toggle.labelled_by(label.id);
                if toggle.changed() {
                    self.journal.model.rearm(Utc::now());
                }
                let interval = ui.add(
                    look::NumberField::new(&mut self.journal.model.interval_minutes)
                        .range(1..=1440)
                        .width(48.0)
                        .steppers(false)
                        .accessible_label("Automatic send interval in minutes"),
                );
                interval.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::SpinButton,
                        enabled,
                        "Automatic send interval in minutes",
                    )
                });
                if interval.changed() {
                    self.journal.model.rearm(Utc::now());
                }
                ui.label("minutes");
            });
        });
        self.send_control(ui, enabled);
        if self.journal.model.uncertain {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Check the receiving chat: delivery is uncertain.",
            );
            ui.horizontal(|ui| {
                if ui.add(look::Button::new("It arrived")).clicked()
                    && let Some(batch) = &self.journal.model.pending
                {
                    let id = batch.id;
                    self.event(Event::SendAccepted { id, at: Utc::now() });
                    self.range = local_time(self.journal.model.range_start);
                }
                if ui.add(look::Button::new("It did not arrive")).clicked()
                    && let Some(batch) = &self.journal.model.pending
                {
                    let id = batch.id;
                    self.event(Event::SendFailed {
                        id,
                        at: Utc::now(),
                        message: "User confirmed non-delivery".into(),
                    });
                }
            });
        }
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).small());
        }
    }
    fn settings_contents(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        let locked = self.recording_since.is_some() || self.is_draining();
        let mut changed = false;
        ui.add_enabled_ui(!locked, |ui| {
            changed |= system_switch(ui, &mut self.settings.mic, "Microphone").changed();
            changed |= device_picker(
                ui,
                "microphone",
                &self.microphones,
                &mut self.settings.microphone,
                self.settings.mic,
            );
            changed |= ui
                .add(look::Checkbox::new(
                    &mut self.settings.output,
                    "System audio",
                ))
                .changed();
            changed |= device_picker(
                ui,
                "speakers",
                &self.speakers,
                &mut self.settings.speakers,
                self.settings.output,
            );
            ui.horizontal(|ui| {
                ui.label("Model");
                let models = [audio::ModelChoice::Nemotron, audio::ModelChoice::Whisper];
                let mut selected = usize::from(self.settings.model == audio::ModelChoice::Whisper);
                let labels: Vec<_> = models.iter().map(|model| model.label()).collect();
                if ui
                    .add(
                        look::Dropdown::new("speech-model", &mut selected, &labels)
                            .accessible_label("Speech model")
                            .width(ui.available_width()),
                    )
                    .changed()
                    && let Some(model) = models.get(selected)
                {
                    self.settings.model = *model;
                    changed = true;
                }
            });
        });
        if changed && let Err(error) = self.save_settings() {
            self.status = error.to_string();
        }
        ui.horizontal(|ui| {
            ui.label("Storage");
            let folder_name = self
                .settings
                .folder
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Choose folder".into());
            if ui
                .add_enabled(
                    !locked && !self.busy,
                    look::Button::new(format!("{folder_name}…")),
                )
                .on_hover_text(self.settings.folder.display().to_string())
                .clicked()
                && let Some(folder) = rfd::FileDialog::new()
                    .set_directory(&self.settings.folder)
                    .pick_folder()
                && let Err(error) = self.change_folder(folder)
            {
                self.status = error.to_string();
            }
        });

        ui.separator();
        ui.label(RichText::new("Global keyboard shortcuts").strong());
        for action in Action::ALL {
            ui.horizontal(|ui| {
                ui.label(action.label());
                let label = if self.listening == Some(action) {
                    "Press keys…".into()
                } else {
                    self.settings.shortcuts.display(action)
                };
                if ui.add(look::Button::new(label)).clicked() {
                    self.hotkeys = None;
                    self.listening = Some(action);
                }
            });
        }
        if self.listening.is_some() {
            ui.label("Press your shortcut. Escape cancels.");
        }
        ui.separator();
        ui.label(RichText::new("Menu bar").strong());
        let changed = system_switch(
            ui,
            &mut self.settings.tray_countdown,
            "Show auto-send countdown",
        )
        .changed()
            | system_switch(
                ui,
                &mut self.settings.tray_levels,
                "Animate input / output levels",
            )
            .changed();
        if changed && let Err(error) = self.save_settings() {
            self.status = error.to_string();
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        if ui.add(look::Button::new("Done")).clicked() {
            self.settings_open = false;
        }
        ui.add_space(8.0);
        if ui.add(look::Button::new("Quit Team-up")).clicked() {
            self.stop();
            self.event(Event::Recording {
                at: Utc::now(),
                active: false,
            });
            self.quitting = true;
            self.status = "Finishing audio before quitting…".into();
        }
    }
    pub fn action(&mut self, action: Action) {
        match action {
            Action::Mute => {
                self.microphone_muted = !self.microphone_muted;
                self.event(Event::MicrophoneMuted {
                    at: Utc::now(),
                    muted: self.microphone_muted,
                });
                if let Some(capture) = &self.capture {
                    capture
                        .muted
                        .store(self.microphone_muted, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Action::Send => self.send(),
            Action::Later | Action::Earlier => {
                if self.journal.model.auto
                    && !self.busy
                    && let Some(at) = &mut self.journal.model.next_send
                {
                    *at = (*at + Duration::minutes(if action == Action::Later { 1 } else { -1 }))
                        .max(Utc::now() + Duration::seconds(1));
                }
            }
        }
    }
    pub fn shortcuts(&mut self, ctx: &egui::Context) {
        let focused = self.test_mode
            || ctx.input(|input| {
                input.viewport().focused == Some(true) && input.viewport().visible() != Some(false)
            });
        if !focused && self.listening.is_some() {
            self.listening = None;
            self.hotkey_error = false;
        }
        if let Some(action) = self.listening {
            for event in ctx.input(|input| input.events.clone()) {
                if let egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                } = event
                {
                    if key == egui::Key::Escape {
                        self.listening = None;
                        self.hotkey_error = false;
                        break;
                    }
                    match crate::shortcuts::from_press(physical_key.unwrap_or(key), modifiers)
                        .and_then(|binding| self.settings.shortcuts.replace(action, binding))
                    {
                        Ok(()) => {
                            self.listening = None;
                            self.hotkey_error = false;
                            if let Err(error) = self.save_settings() {
                                self.status = error.to_string();
                            }
                        }
                        Err(error) => self.status = error.to_string(),
                    }
                    break;
                }
            }
            return;
        }
        if !self.test_mode && self.hotkeys.is_none() && !self.hotkey_error {
            match Registry::new(&self.settings.shortcuts) {
                Ok(registry) => self.hotkeys = Some(registry),
                Err(error) => {
                    self.status = format!("Shortcut unavailable: {error}");
                    self.hotkey_error = true;
                }
            }
        }
        let mut actions = Vec::new();
        while let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().try_recv() {
            if event.state == global_hotkey::HotKeyState::Pressed {
                let action = self.hotkeys.as_ref().and_then(|registry| {
                    registry
                        .registered
                        .iter()
                        .find(|(_, key)| key.id == event.id)
                        .map(|(action, _)| *action)
                });
                if let Some(action) = action {
                    actions.push(action);
                }
            }
        }
        // Also support focused-window key events. Native automation and some
        // keyboard services send these directly to the app instead of Carbon.
        for event in ctx.input(|input| {
            if focused {
                input.events.clone()
            } else {
                vec![]
            }
        }) {
            if let egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat: false,
                modifiers,
            } = event
                && let Ok(binding) =
                    crate::shortcuts::from_press(physical_key.unwrap_or(key), modifiers)
                && let Ok(key) = binding.parse::<global_hotkey::hotkey::HotKey>()
                && let Ok(bindings) = self.settings.shortcuts.parsed()
                && let Some((action, _)) = bindings
                    .into_iter()
                    .find(|(_, registered)| registered.id == key.id)
                && !actions.contains(&action)
            {
                actions.push(action);
            }
        }
        for action in actions {
            self.action(action);
        }
    }

    fn picker(&mut self, ui: &mut egui::Ui) {
        ui.label("Codex chats");
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .max_height(145.0)
            .show(ui, |ui| {
                for chat in &self.chats {
                    if ui.selectable_label(false, &chat.title).clicked() {
                        chosen = Some(chat.clone());
                    }
                }
            });
        if let Some(chat) = chosen {
            self.choose(chat);
        }
        ui.horizontal(|ui| {
            ui.add(
                look::TextField::new(&mut self.new_title)
                    .id_salt("new-chat-title")
                    .hint("New chat title")
                    .accessible_label("New chat title")
                    .width(190.0),
            )
            .widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "New chat title")
            });
            if ui
                .add_enabled(
                    !self.busy && !self.new_title.trim().is_empty(),
                    look::Button::new("Create"),
                )
                .clicked()
            {
                self.busy = true;
                let title = self.new_title.clone();
                let tx = self.work_tx.clone();
                thread::spawn(move || {
                    let _ = tx.send(Work::Created(
                        delivery::create(&title).map_err(|e| e.to_string()),
                    ));
                });
            }
        });
        if ui.add(look::Button::new("Cancel")).clicked() {
            self.choosing = false;
        }
    }
    fn send_control(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let now = Utc::now();
        let elapsed = self
            .journal
            .model
            .last_sent
            .map(|at| format!("{} min since last send", (now - at).num_minutes().max(0)))
            .unwrap_or_else(|| "Not sent yet".into());
        let automatic = self.journal.model.auto && self.journal.model.destination.is_some();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 68.0), egui::Sense::hover());
        let send_rect = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(
                rect.right() - if automatic { 112.0 } else { 0.0 },
                rect.bottom(),
            ),
        );
        let response = ui.interact(
            send_rect,
            ui.scope_id().with("send-now"),
            if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, "Send now"));
        let colors = look::colors(ui);
        ui.painter().rect_filled(rect, 8.0, colors.button);
        look::paint_button(
            ui.painter(),
            send_rect,
            &response,
            "",
            egui::FontId::proportional(13.0),
            enabled,
            true,
        );
        let label = if self.busy { "Sending…" } else { "Send now" };
        ui.painter().text(
            send_rect.min + egui::vec2(16.0, 22.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(16.0),
            if enabled {
                colors.on_accent
            } else {
                colors.disabled
            },
        );
        ui.painter().text(
            send_rect.min + egui::vec2(16.0, 44.0),
            egui::Align2::LEFT_CENTER,
            elapsed,
            egui::FontId::proportional(11.0),
            if enabled {
                colors.on_accent.gamma_multiply(0.8)
            } else {
                colors.disabled
            },
        );
        if response.clicked() && enabled {
            self.send();
        }
        if automatic {
            let remaining = self
                .journal
                .model
                .next_send
                .map(|at| (at - now).num_seconds().max(0))
                .unwrap_or(0);
            let left = send_rect.right() + 12.0;
            ui.painter().text(
                egui::pos2(left, rect.top() + 19.0),
                egui::Align2::LEFT_CENTER,
                "Next send",
                egui::FontId::proportional(10.0),
                colors.dim,
            );
            ui.painter().text(
                egui::pos2(left, rect.top() + 40.0),
                egui::Align2::LEFT_CENTER,
                format!("{}:{:02}", remaining / 60, remaining % 60),
                egui::FontId::proportional(20.0),
                colors.fg,
            );
            for (index, label, adjustment) in [(0, "+", 1), (1, "-", -1)] {
                let button = egui::Rect::from_min_size(
                    egui::pos2(rect.right() - 28.0, rect.top() + 6.0 + index as f32 * 26.0),
                    egui::vec2(24.0, 24.0),
                );
                let response = ui.put(button, look::Button::new(label).width(24.0));
                if enabled
                    && response.clicked()
                    && let Some(at) = &mut self.journal.model.next_send
                {
                    *at = (*at + Duration::minutes(adjustment))
                        .max(Utc::now() + Duration::seconds(1));
                }
            }
            let fraction = remaining as f32
                / (self.journal.model.interval_minutes as f32 * 60.0)
                    .max(remaining as f32)
                    .max(1.0);
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - 3.0),
                    egui::pos2(rect.left() + rect.width() * fraction, rect.bottom()),
                ),
                1.0,
                colors.accent,
            );
        }
    }
    fn tray(&mut self, ctx: &egui::Context) {
        let state = if !self.status.is_empty() && self.recording_since.is_none() {
            "error"
        } else if self.recording_since.is_some() {
            "recording"
        } else {
            "stopped"
        };
        if self.tray.is_none() {
            let context = ctx.clone();
            let panel_visible = self.panel_visible.clone();
            TrayIconEvent::set_event_handler(Some(move |event| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: tray_icon::MouseButton::Left,
                        button_state: tray_icon::MouseButtonState::Up,
                        ..
                    }
                ) {
                    if let TrayIconEvent::Click { position, .. } = event {
                        let scale = f64::from(context.native_pixels_per_point().unwrap_or(1.0));
                        context.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                            egui::pos2(
                                (position.x / scale - 340.0).max(0.0) as f32,
                                (position.y / scale + 16.0) as f32,
                            ),
                        ));
                    }
                    let visible =
                        !panel_visible.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                    context.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
                    if visible {
                        context.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    context.request_repaint();
                }
            }));
            match tray_icon(state, self.meters).and_then(|icon| {
                Ok(TrayIconBuilder::new()
                    .with_icon(icon)
                    .with_tooltip("Team-up")
                    .build()?)
            }) {
                Ok(tray) => self.tray = Some(tray),
                Err(error) => self.status = format!("Menu bar unavailable: {error}"),
            }
        }
        let measured = self
            .capture
            .as_ref()
            .map(|capture| capture.levels.values())
            .unwrap_or((0.0, 0.0));
        self.meters = if self.settings.tray_levels && self.recording_since.is_some() {
            (
                (measured.0 * 8.0).clamp(0.0, 1.0).max(self.meters.0 * 0.72),
                (measured.1 * 8.0).clamp(0.0, 1.0).max(self.meters.1 * 0.72),
            )
        } else {
            (0.0, 0.0)
        };
        if let Some(tray) = &self.tray {
            let countdown = if self.settings.tray_countdown && self.journal.model.auto {
                self.journal
                    .model
                    .next_send
                    .map(|at| {
                        let seconds = (at - Utc::now()).num_seconds().max(0);
                        format!("{}:{:02}", seconds / 60, seconds % 60)
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            };
            tray.set_title(Some(countdown));
        }
        if self.tray_state != state || self.recording_since.is_some() {
            if let Some(tray) = &self.tray
                && let Ok(icon) = tray_icon(state, self.meters)
            {
                let _ = tray.set_icon(Some(icon));
            }
            self.tray_state = state.into();
        }
    }
}
impl eframe::App for Panel {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick();
        self.shortcuts(ctx);
        self.tray(ctx);
        if self.quitting && self.capture.as_ref().is_none_or(Capture::finished) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(StdDuration::from_millis(200));
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if (!self.quitting
            || self
                .capture
                .as_ref()
                .is_some_and(|capture| !capture.finished()))
            && ui.ctx().input(|i| i.viewport().close_requested())
        {
            // Explicit Quit bypasses this via the quit flag added by the native runner.
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        let content = look::window_frame(ui.ctx())
            .inner_margin(16.0)
            .show(ui, |ui| {
                self.contents(ui);
                ui.min_rect().height()
            });
        let height = (content.inner + 32.0).ceil().clamp(280.0, 850.0);
        if (height - self.window_height).abs() > 1.0 {
            self.window_height = height;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(364.0, height)));
            ui.ctx().request_repaint();
        }
    }
}
fn local_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}
fn system_switch(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let label_response = ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(look::Switch::new(value).accessible_label(label))
        })
        .inner
        .labelled_by(label_response.id)
    })
    .inner
}

fn device_picker(
    ui: &mut egui::Ui,
    id: &str,
    names: &[String],
    selection: &mut String,
    enabled: bool,
) -> bool {
    let previous = selection.clone();
    let mut items = vec!["System default"];
    items.extend(names.iter().map(String::as_str));
    let mut index = names
        .iter()
        .position(|name| name == selection)
        .map(|index| index + 1)
        .unwrap_or(0);
    ui.add_enabled_ui(enabled, |ui| {
        if ui
            .add(
                look::Dropdown::new(id, &mut index, &items)
                    .accessible_label(id)
                    .width(ui.available_width()),
            )
            .changed()
        {
            *selection = index
                .checked_sub(1)
                .and_then(|index| names.get(index))
                .cloned()
                .unwrap_or_default();
        }
    });
    *selection != previous
}
fn tray_icon(state: &str, levels: (f32, f32)) -> Result<tray_icon::Icon> {
    let mut bytes = vec![0u8; 22 * 22 * 4];
    let color = match state {
        "recording" => [199, 43, 59, 255],
        "error" => [210, 130, 20, 255],
        _ => [110, 115, 124, 255],
    };
    for (index, level) in [levels.0 * 0.7, levels.0, levels.1, levels.1 * 0.7]
        .into_iter()
        .enumerate()
    {
        let height = 3 + (level * 15.0) as usize;
        for y in (22 - height) / 2..(22 + height) / 2 {
            for x in 3 + index * 4..6 + index * 4 {
                bytes[(y * 22 + x) * 4..(y * 22 + x + 1) * 4].copy_from_slice(&color);
            }
        }
    }
    Ok(tray_icon::Icon::from_rgba(bytes, 22, 22)?)
}
