#![forbid(unsafe_code)]
use anyhow::{Result, ensure};
use std::{path::Path, sync::mpsc};
use team_up::{
    audio::{AudioEvent, Runtime, stream_wav},
    journal::Journal,
    ui::{Panel, Settings},
};
#[test]
#[ignore = "loads the installed local speech model and streams real audio at wall-clock speed"]
fn real_streaming_audio_reaches_app_and_survives_restart() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let settings = Settings {
        folder: directory.path().to_path_buf(),
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
    let runtime = Runtime::start()?;
    let (tx, rx) = mpsc::channel();
    stream_wav(runtime.port, Path::new("fixtures/asr/spoken.wav"), tx)?;
    for event in rx.try_iter() {
        match event {
            AudioEvent::Transcript(segment) => app.ingest(segment),
            AudioEvent::Error(error) => anyhow::bail!(error),
            AudioEvent::Ready | AudioEvent::Flushed { .. } => {}
        }
    }
    ensure!(
        !app.journal.model.segments.is_empty(),
        "Real ASR returned no finalized speech"
    );
    let text = app
        .journal
        .model
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    println!("Real local ASR transcript: {text}");
    ensure!(
        text.split_whitespace().count() >= 4,
        "Insufficient speech recognized"
    );
    let count = app.journal.model.segments.len();
    drop(app);
    let recovered = Journal::open(directory.path())?;
    ensure!(
        recovered.model.segments.len() == count,
        "Transcript recovery mismatch"
    );
    Ok(())
}

#[test]
#[ignore = "plays speech through the real output device and requires macOS audio-capture permission"]
fn actual_output_device_streams_to_model_and_retains_audio_only_when_enabled() -> Result<()> {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    use team_up::audio::{Capture, CaptureOptions, ModelChoice};
    let directory = tempfile::tempdir()?;
    let runtime = Runtime::start()?;
    let (tx, rx) = mpsc::channel();
    let mut capture = Capture::start(
        runtime.port,
        vec![("system audio".into(), true, String::new())],
        CaptureOptions {
            model: ModelChoice::Nemotron,
            audio_directory: Some(directory.path().into()),
            retain_audio: true,
            microphone_muted: false,
        },
        tx,
    );
    loop {
        match rx.recv_timeout(Duration::from_secs(100))? {
            AudioEvent::Ready => break,
            AudioEvent::Error(error) => anyhow::bail!(error),
            AudioEvent::Transcript(_) | AudioEvent::Flushed { .. } => {}
        }
    }
    let result = Command::new("/opt/homebrew/bin/ffplay")
        .args([
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "error",
            "fixtures/asr/spoken.wav",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    ensure!(result.status.success(), "Fixture playback failed");
    // Speech must finalize while capture is still running. A stop-only test
    // missed the real bug where Send had nothing finalized to deliver.
    let live_deadline = Instant::now() + Duration::from_secs(15);
    let mut transcript = String::new();
    while Instant::now() < live_deadline && !transcript.to_lowercase().contains("meeting") {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(AudioEvent::Transcript(segment)) => {
                transcript.push_str(&segment.text);
                transcript.push(' ');
            }
            Ok(AudioEvent::Error(error)) => anyhow::bail!(error),
            _ => {}
        }
    }
    ensure!(
        !capture.finished(),
        "Capture stopped before the live transcript arrived"
    );
    ensure!(
        transcript.to_lowercase().contains("meeting"),
        "No finalized live speech before stopping: {transcript}"
    );
    let generation = capture.flush();
    let flush_deadline = Instant::now() + Duration::from_secs(20);
    let mut acknowledged = false;
    while Instant::now() < flush_deadline && !acknowledged {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(AudioEvent::Flushed {
                generation: ack, ..
            }) => acknowledged = ack == generation,
            Ok(AudioEvent::Transcript(segment)) => {
                transcript.push_str(&segment.text);
                transcript.push(' ');
            }
            Ok(AudioEvent::Error(error)) => anyhow::bail!(error),
            _ => {}
        }
    }
    ensure!(acknowledged, "Send-time audio flush was not acknowledged");
    capture.stop();
    let until = Instant::now() + Duration::from_secs(40);
    while Instant::now() < until {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(AudioEvent::Transcript(segment)) => {
                transcript.push_str(&segment.text);
                transcript.push(' ');
            }
            Ok(AudioEvent::Error(error)) => anyhow::bail!(error),
            _ => {}
        }
        if capture.finished() {
            break;
        }
    }
    ensure!(capture.finished(), "Capture did not shut down cleanly");
    ensure!(
        transcript.to_lowercase().contains("meeting"),
        "Actual output capture did not recognize the spoken meeting fixture: {transcript}"
    );
    let files = std::fs::read_dir(directory.path())?.collect::<std::io::Result<Vec<_>>>()?;
    ensure!(files.len() == 1, "Expected one retained audio file");
    let wav = hound::WavReader::open(files[0].path())?;
    ensure!(wav.duration() > 16000, "Retained audio is empty");
    println!("Actual output-device transcript: {transcript}");
    Ok(())
}

#[test]
#[ignore = "runs the installed Whisper model on real streamed audio"]
fn whisper_streams_real_audio_without_retaining_audio() -> Result<()> {
    use team_up::audio::{ModelChoice, stream_wav_with_model};
    let directory = tempfile::tempdir()?;
    let settings = Settings {
        folder: directory.path().into(),
        mic: true,
        output: false,
        microphone: String::new(),
        speakers: String::new(),
        model: ModelChoice::Whisper,
        keep_audio: false,
        shortcuts: Default::default(),
        tray_countdown: true,
        tray_levels: true,
    };
    let mut app = Panel::with_journal(Journal::open(directory.path())?, settings, true);
    let (tx, rx) = mpsc::channel();
    stream_wav_with_model(
        0,
        Path::new("fixtures/asr/spoken.wav"),
        ModelChoice::Whisper,
        tx,
    )?;
    for event in rx.try_iter() {
        if let AudioEvent::Transcript(segment) = event {
            app.ingest(segment);
        }
    }
    let text = app
        .journal
        .model
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    ensure!(
        text.to_lowercase().contains("meeting"),
        "Whisper did not recognize the fixture: {text}"
    );
    ensure!(
        !directory.path().join("audio").exists(),
        "Audio was retained despite default-off setting"
    );
    println!("Whisper transcript: {text}");
    Ok(())
}
