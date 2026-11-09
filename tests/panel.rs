#![forbid(unsafe_code)]
use anyhow::Result;
use egui_kittest::{Harness, kittest::Queryable};
use team_up::{
    journal::Journal,
    model::Destination,
    ui::{Panel, Settings},
};
#[test]
fn native_controls_record_choose_chat_and_toggle_automatic_sends() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let settings = Settings {
        folder: directory.path().into(),
        mic: true,
        output: false,
        microphone: String::new(),
        speakers: String::new(),
        model: Default::default(),
        keep_audio: false,
        shortcuts: Default::default(),
        tray_countdown: true,
        tray_levels: true,
    };
    let app = Panel::with_journal(Journal::open(directory.path())?, settings, true);
    let mut harness = Harness::builder()
        .with_size(eframe::egui::vec2(380.0, 800.0))
        .build_ui_state(|ui, panel: &mut Panel| panel.contents(ui), app);
    harness.get_by_label("Start recording").click();
    harness.run();
    assert!(harness.state().recording_since.is_some());
    harness.get_by_label("Choose a chat…").click();
    harness.run();
    assert!(harness.state().choosing);
    harness.state_mut().choose(Destination {
        provider: "test".into(),
        id: "meeting".into(),
        title: "Meeting".into(),
    });
    harness.run();
    harness.get_by_label("Auto-send every").click();
    harness.run();
    assert!(harness.state().journal.model.auto);
    let before = harness.state().journal.model.next_send;
    harness.get_by_label("+").click();
    harness.run();
    assert_eq!(
        harness.state().journal.model.next_send,
        before.map(|at| at + chrono::Duration::minutes(1))
    );
    assert_eq!(harness.state().journal.model.interval_minutes, 7);
    harness.get_by_label("-").click();
    harness.run();
    assert_eq!(harness.state().journal.model.next_send, before);
    harness.get_by_label("Auto-send every").click();
    harness.run();
    assert!(!harness.state().journal.model.auto);
    assert!(harness.state().recording_since.is_some());
    Ok(())
}

#[test]
fn settings_icon_moves_configuration_and_mute_does_not_stop_recording() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let settings = Settings {
        folder: directory.path().into(),
        mic: true,
        output: false,
        microphone: String::new(),
        speakers: String::new(),
        model: Default::default(),
        keep_audio: false,
        shortcuts: Default::default(),
        tray_countdown: true,
        tray_levels: true,
    };
    let app = Panel::with_journal(Journal::open(directory.path())?, settings, true);
    let mut harness = Harness::builder()
        .with_size(eframe::egui::vec2(380.0, 850.0))
        .build_ui_state(|ui, panel: &mut Panel| panel.contents(ui), app);
    harness.get_by_label("Start recording").click();
    harness.run();
    let started = harness.state().recording_since;
    harness.get_by_label("Mute microphone").click();
    harness.run();
    assert!(harness.state().microphone_muted);
    assert_eq!(harness.state().recording_since, started);
    harness.get_by_label("Settings").click();
    harness.run();
    assert!(harness.state().settings_open);
    harness.get_by_label("Show auto-send countdown").click();
    harness.run();
    assert!(!harness.state().settings.tray_countdown);
    harness.get_by_label("Done").click();
    harness.run();
    assert!(!harness.state().settings_open);
    harness.get_by_label("Unmute microphone").click();
    harness.run();
    assert!(!harness.state().microphone_muted);
    assert_eq!(harness.state().recording_since, started);
    Ok(())
}

#[test]
fn recorded_shortcut_triggers_only_its_action() -> Result<()> {
    use eframe::egui::{Key, Modifiers};
    let directory = tempfile::tempdir()?;
    let settings = Settings {
        folder: directory.path().into(),
        mic: true,
        output: false,
        microphone: String::new(),
        speakers: String::new(),
        model: Default::default(),
        keep_audio: false,
        shortcuts: Default::default(),
        tray_countdown: true,
        tray_levels: true,
    };
    let app = Panel::with_journal(Journal::open(directory.path())?, settings, true);
    let mut harness = Harness::builder()
        .with_size(eframe::egui::vec2(380.0, 850.0))
        .build_ui_state(
            |ui, panel: &mut Panel| {
                panel.shortcuts(ui.ctx());
                panel.contents(ui);
            },
            app,
        );
    harness.get_by_label("Settings").click();
    harness.run();
    harness.get_by_label("Cmd + Shift + M").click();
    harness.run();
    let modifiers = Modifiers {
        mac_cmd: true,
        command: true,
        shift: true,
        ..Modifiers::NONE
    };
    harness.key_press_modifiers(modifiers, Key::K);
    harness.run();
    assert!(harness.state().listening.is_none());
    assert!(
        !harness.state().microphone_muted,
        "Recording a shortcut must not execute it"
    );
    harness.get_by_label("Done").click();
    harness.run();
    harness.key_press_modifiers(modifiers, Key::K);
    harness.run();
    assert!(harness.state().microphone_muted);
    harness.key_press_modifiers(modifiers, Key::K);
    harness.run();
    assert!(!harness.state().microphone_muted);
    assert!(harness.state().journal.model.last_sent.is_none());
    Ok(())
}

#[test]
fn changing_storage_preserves_transcript_and_optional_audio() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let destination = tempfile::tempdir()?;
    let settings = Settings {
        folder: directory.path().into(),
        mic: true,
        output: false,
        microphone: String::new(),
        speakers: String::new(),
        model: Default::default(),
        keep_audio: false,
        shortcuts: Default::default(),
        tray_countdown: true,
        tray_levels: true,
    };
    let mut app = Panel::with_journal(Journal::open(directory.path())?, settings, true);
    let now = chrono::Utc::now();
    app.ingest(team_up::model::Segment {
        id: uuid::Uuid::new_v4(),
        source: "microphone".into(),
        start: now,
        end: now,
        text: "saved speech".into(),
    });
    std::fs::create_dir(directory.path().join("audio"))?;
    let mut writer = hound::WavWriter::create(
        directory.path().join("audio/retained.wav"),
        hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    writer.write_sample(32i16)?;
    writer.finalize()?;
    app.change_folder(destination.path().into())?;
    assert!(destination.path().join("audio/retained.wav").is_file());
    assert_eq!(Journal::open(destination.path())?.model.segments.len(), 1);
    assert_eq!(app.settings.folder, destination.path());
    Ok(())
}
