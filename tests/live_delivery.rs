#![forbid(unsafe_code)]
use anyhow::{Result, ensure};
use egui_kittest::{Harness, kittest::Queryable};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use team_up::{
    delivery,
    journal::Journal,
    ui::{Panel, Settings},
};

#[test]
#[ignore = "operates the real UI, captures real output audio, and sends it to a dedicated Codex chat; requires TEAM_UP_LIVE_DELIVERY_E2E=1"]
fn real_output_audio_is_sent_before_recording_stops_and_visible_in_receiving_chat() -> Result<()> {
    ensure!(
        std::env::var("TEAM_UP_LIVE_DELIVERY_E2E").as_deref() == Ok("1"),
        "Live delivery requires explicit opt-in"
    );
    let directory = tempfile::tempdir()?;
    let settings = Settings {
        folder: directory.path().into(),
        mic: false,
        output: true,
        microphone: String::new(),
        speakers: String::new(),
        model: Default::default(),
        keep_audio: false,
        shortcuts: Default::default(),
        tray_countdown: true,
        tray_levels: true,
    };
    let title = format!(
        "Team-up E2E audio {}",
        chrono::Utc::now().format("%H:%M:%S")
    );
    let app = Panel::with_journal(Journal::open(directory.path())?, settings, false);
    let mut ui = Harness::builder()
        .with_size(eframe::egui::vec2(364.0, 850.0))
        .build_ui_state(
            |ui, app: &mut Panel| {
                app.tick();
                app.contents(ui);
            },
            app,
        );
    ui.ctx.add_font_provider(std::sync::Arc::new(
        egui_system_font_provider::SystemFontProvider::new(),
    ));
    team_up::theme::apply(&ui.ctx);
    ui.run_steps(2);
    ui.get_by_label("Choose a chat…").click();
    ui.run();
    let picker_deadline = Instant::now() + Duration::from_secs(25);
    while ui.state().busy && Instant::now() < picker_deadline {
        ui.run_steps(1);
        std::thread::sleep(Duration::from_millis(25));
    }
    ui.get_by_label("New chat title").click();
    ui.run();
    ui.get_by_label("New chat title").type_text(&title);
    ui.run();
    ensure!(
        ui.state().new_title == title,
        "Chat title input did not accept typing"
    );
    ui.get_by_label("Create").click();
    ui.run();
    let create_deadline = Instant::now() + Duration::from_secs(25);
    while ui.state().busy && Instant::now() < create_deadline {
        ui.run_steps(1);
        std::thread::sleep(Duration::from_millis(25));
    }
    let chat = ui
        .state()
        .journal
        .model
        .destination
        .clone()
        .ok_or_else(|| anyhow::anyhow!("New chat creation failed: {}", ui.state().status))?;
    ensure!(
        chat.title == title,
        "Chat picker selected the wrong destination"
    );
    ui.get_by_label("Open in Codex").click();
    ui.run();
    let owner_deadline = Instant::now() + Duration::from_secs(30);
    let mut owned = false;
    while Instant::now() < owner_deadline {
        if team_up::desktop::Desktop::discover(&team_up::desktop::socket_path()?, &chat.id)?
            .is_some()
        {
            owned = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    ensure!(owned, "Codex did not open and own the destination chat");
    ui.get_by_label("Start recording").click();
    ui.run();
    let ready_deadline = Instant::now() + Duration::from_secs(90);
    while ui.state().status.contains("Starting") && Instant::now() < ready_deadline {
        ui.run_steps(1);
        std::thread::sleep(Duration::from_millis(25));
    }
    ensure!(
        ui.state().recording_since.is_some() && ui.state().status.is_empty(),
        "Capture failed to start: {}",
        ui.state().status
    );
    let mut player = Command::new("/opt/homebrew/bin/ffplay")
        .args([
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "error",
            "fixtures/asr/spoken.wav",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    while player.try_wait()?.is_none() {
        ui.run_steps(1);
        std::thread::sleep(Duration::from_millis(25));
    }
    ensure!(player.wait()?.success(), "Audio fixture playback failed");
    ui.get_by_label("Send now").click();
    ui.run();
    let delivery_deadline = Instant::now() + Duration::from_secs(45);
    while ui.state().busy && Instant::now() < delivery_deadline {
        ui.run_steps(1);
        std::thread::sleep(Duration::from_millis(25));
    }
    ensure!(
        ui.state().recording_since.is_some(),
        "Sending stopped the recording: {}",
        ui.state().status
    );
    ensure!(
        ui.state().journal.model.last_sent.is_some(),
        "Real delivery failed: {}; finalized segments: {}; timestamps relative to now (start,end): {:?}",
        ui.state().status,
        ui.state().journal.model.segments.len(),
        ui.state()
            .journal
            .model
            .segments
            .iter()
            .map(|segment| (
                (segment.start - chrono::Utc::now()).num_milliseconds(),
                (segment.end - chrono::Utc::now()).num_milliseconds()
            ))
            .collect::<Vec<_>>()
    );
    ensure!(
        !ui.state().journal.model.uncertain,
        "Delivery was uncertain"
    );
    let verify_deadline = Instant::now() + Duration::from_secs(40);
    let mut received = false;
    let mut agent_reacted = false;
    while Instant::now() < verify_deadline {
        let result = delivery::read_chat(&chat.id)?;
        let serialized = result.to_string().to_lowercase();
        received |= serialized.contains("meeting starts");
        agent_reacted |= result["thread"]["turns"].as_array().is_some_and(|turns| {
            turns.iter().any(|turn| {
                turn["items"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["type"] == "agentMessage"))
            })
        });
        if received && agent_reacted {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    ensure!(
        received,
        "Receiving chat did not contain the actual speech transcript"
    );
    ensure!(
        agent_reacted,
        "Receiving agent did not respond to the real transcript"
    );
    let first_send = ui.state().journal.model.last_sent;
    ui.get_by_label("Auto-send every").click();
    ui.run();
    ui.get_by_label("+").click();
    ui.run();
    ensure!(
        ui.state().journal.model.interval_minutes == 7,
        "Adjusting deadline changed the interval"
    );
    let mut player = Command::new("/opt/homebrew/bin/ffplay")
        .args([
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "error",
            "fixtures/asr/spoken.wav",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    while player.try_wait()?.is_none() {
        ui.run_steps(1);
        std::thread::sleep(Duration::from_millis(25));
    }
    ensure!(player.wait()?.success(), "Second fixture playback failed");
    // Use the actual public minus control to bring the next 8-minute deadline
    // forward; do not replace time or invoke the sending service directly.
    for _ in 0..8 {
        ui.get_by_label("-").click();
        ui.run();
    }
    let automatic_deadline = Instant::now() + Duration::from_secs(45);
    while ui.state().journal.model.last_sent == first_send && Instant::now() < automatic_deadline {
        ui.run_steps(1);
        std::thread::sleep(Duration::from_millis(25));
    }
    ensure!(
        ui.state().journal.model.last_sent != first_send,
        "Automatic send did not acknowledge: {}; segments (start,end,delivered) relative to now: {:?}",
        ui.state().status,
        ui.state()
            .journal
            .model
            .segments
            .iter()
            .map(|s| (
                (s.start - chrono::Utc::now()).num_milliseconds(),
                (s.end - chrono::Utc::now()).num_milliseconds(),
                ui.state().journal.model.delivered.contains(&s.id)
            ))
            .collect::<Vec<_>>()
    );
    ensure!(
        ui.state().recording_since.is_some(),
        "Automatic sending stopped capture: {}",
        ui.state().status
    );
    let readback = delivery::read_chat(&chat.id)?;
    let messages = readback["thread"]["turns"]
        .as_array()
        .map(|turns| {
            turns
                .iter()
                .map(|turn| {
                    turn["items"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter(|item| item["type"] == "userMessage")
                                .count()
                        })
                        .unwrap_or(0)
                })
                .sum::<usize>()
        })
        .unwrap_or(0);
    ensure!(
        messages >= 2,
        "The second automatic transcript was not in the same receiving chat"
    );
    ui.state_mut().toggle_recording();
    let stop_deadline = Instant::now() + Duration::from_secs(30);
    while ui.state().is_draining() && Instant::now() < stop_deadline {
        ui.run_steps(1);
        std::thread::sleep(Duration::from_millis(25));
    }
    ensure!(
        !ui.state().is_draining(),
        "Capture failed to finish cleanly"
    );
    println!(
        "UI chat picker → UI Start → actual system audio → local ASR → UI Send → receiving Codex chat → actual agent response → automatic second send to the same chat verified."
    );
    Ok(())
}
