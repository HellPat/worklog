use anyhow::{Result, ensure};
use std::{path::Path, sync::mpsc};
use team_up::audio::{AudioEvent, Runtime, stream_wav};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "Usage: spike_audio_fixture WAV OUTPUT_JSON"
    );
    let runtime = Runtime::start()?;
    let (tx, rx) = mpsc::channel();
    stream_wav(runtime.port, Path::new(&args[1]), tx)?;
    let mut segments = Vec::new();
    for event in rx.try_iter() {
        match event {
            AudioEvent::Transcript(s) => segments.push(s),
            AudioEvent::Error(e) => anyhow::bail!(e),
            _ => {}
        }
    }
    ensure!(!segments.is_empty(), "Real audio yielded no transcript");
    let origin = segments[0].start;
    let base =
        chrono::DateTime::parse_from_rfc3339("2026-11-09T14:30:00+01:00")?.timestamp_millis();
    let output: Vec<_> = segments
        .iter()
        .map(|s| serde_json::json!({"time":base+(s.start-origin).num_milliseconds(),"text":s.text}))
        .collect();
    std::fs::write(&args[2], serde_json::to_vec_pretty(&output)?)?;
    println!("Transcribed {} segments from {}", output.len(), args[1]);
    Ok(())
}
