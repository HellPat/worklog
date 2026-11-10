//! The GPUI app shares the local audio/transcription core with the existing app.
use crate::{
    audio_settings::{AudioSettings, SYSTEM_DEFAULT},
    local_audio::{self, AudioEvent, Capture, CaptureOptions, ModelChoice, Runtime},
};
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

pub struct Session {
    source: Source,
    events: Receiver<AudioEvent>,
    _runtime: Option<Runtime>,
    readiness: Readiness,
    failed_input: bool,
    failed_output: bool,
}
pub const RECORD_BUTTON_WIDTH: f32 = 144.;
#[derive(Clone, Debug)]
pub struct CaptureProblem {
    pub summary: String,
    pub help: String,
    pub details: String,
}
impl CaptureProblem {
    pub fn from_error(error: &str) -> Self {
        let (summary, help) = if error.starts_with("system audio:") {
            (
                "System audio is unavailable",
                "Check the output device in Settings and the macOS Screen & System Audio Recording permission.",
            )
        } else if error.starts_with("microphone:") {
            (
                "Microphone audio is unavailable",
                "Check the selected microphone in Settings and allow Microphone access in macOS Privacy & Security.",
            )
        } else {
            (
                "Recording could not start",
                "Check the selected audio devices and local transcription engine in Settings.",
            )
        };
        Self {
            summary: summary.into(),
            help: help.into(),
            details: error.into(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Idle,
    Starting,
    OnAir,
    Finishing,
}
impl ButtonState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Record",
            Self::Starting => "Starting…",
            Self::OnAir => "Stop",
            Self::Finishing => "Finishing…",
        }
    }
}
#[derive(Default)]
struct Readiness {
    pending: BTreeSet<String>,
    failed: bool,
    announced: bool,
}
impl Readiness {
    fn new(sources: &[&str]) -> Self {
        Self {
            pending: sources.iter().map(|source| (*source).into()).collect(),
            ..Default::default()
        }
    }
    fn ready(&self) -> bool {
        self.pending.is_empty() && !self.failed
    }
    fn source_ready(&mut self, source: &str) {
        self.pending.remove(source);
    }
    fn take_cue(&mut self) -> bool {
        if self.ready() && !self.announced {
            self.announced = true;
            true
        } else {
            false
        }
    }
}
enum Source {
    Live(Capture),
    Fixture {
        worker: thread::JoinHandle<()>,
        stop: Arc<AtomicBool>,
        levels: Arc<local_audio::Levels>,
    },
}
impl Source {
    fn stop(&mut self) {
        match self {
            Self::Live(capture) => capture.stop(),
            Self::Fixture { stop, .. } => stop.store(true, Ordering::Relaxed),
        }
    }
    fn finished(&self) -> bool {
        match self {
            Self::Live(capture) => capture.finished(),
            Self::Fixture { worker, .. } => worker.is_finished(),
        }
    }
    fn levels(&self) -> (f32, f32) {
        match self {
            Self::Live(capture) => capture.levels.values(),
            Self::Fixture { levels, .. } => levels.values(),
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.source.stop();
    }
}
#[derive(Default)]
pub enum Recording {
    #[default]
    Idle,
    Running(Session),
    Finishing(Session),
}
impl Recording {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running(_))
    }
    pub fn is_finishing(&self) -> bool {
        matches!(self, Self::Finishing(_))
    }
    pub fn is_starting(&self) -> bool {
        matches!(self, Self::Running(session) if !session.readiness.ready())
    }
    pub fn button_state(&self) -> ButtonState {
        match self {
            Self::Idle => ButtonState::Idle,
            Self::Finishing(_) => ButtonState::Finishing,
            Self::Running(session) if session.readiness.ready() => ButtonState::OnAir,
            _ => ButtonState::Starting,
        }
    }
    pub fn take_on_air_cue(&mut self) -> bool {
        match self {
            Self::Running(session) => session.readiness.take_cue(),
            _ => false,
        }
    }
    pub fn toggle(
        &mut self,
        devices: &AudioSettings,
        model: ModelChoice,
        directory: &Path,
    ) -> Result<()> {
        *self = match std::mem::take(self) {
            Self::Running(mut session) => {
                session.source.stop();
                Self::Finishing(session)
            }
            Self::Finishing(session) => {
                *self = Self::Finishing(session);
                bail!("Finishing the previous recording");
            }
            Self::Idle => Self::Running(Session::start(devices, model, directory)?),
        };
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<AudioEvent> {
        let (events, finished, failed) = match self {
            Self::Idle => return vec![],
            Self::Running(session) | Self::Finishing(session) => {
                let mut events = session.events.try_iter().collect::<Vec<_>>();
                let mut fatal = false;
                for event in &events {
                    match event {
                        AudioEvent::SourceReady { source } => {
                            session.readiness.source_ready(source)
                        }
                        AudioEvent::Ready if matches!(session.source, Source::Fixture { .. }) => {
                            session.readiness.source_ready("fixture")
                        }
                        AudioEvent::Error(error) => {
                            if !session.readiness.announced {
                                session.readiness.failed = true;
                                fatal = true;
                            }
                            if error.starts_with("system audio:") {
                                session.failed_output = true;
                            } else if error.starts_with("microphone:") {
                                session.failed_input = true;
                            } else {
                                fatal = true;
                            }
                        }
                        _ => {}
                    }
                }
                fatal |= session.failed_input && session.failed_output;
                if fatal {
                    session.source.stop();
                }
                let finished = session.source.finished();
                if finished {
                    events.extend(session.events.try_iter());
                }
                (events, finished, fatal)
            }
        };
        if finished {
            *self = Self::Idle;
        } else if failed && self.is_running() {
            if let Self::Running(session) = std::mem::take(self) {
                *self = Self::Finishing(session);
            }
        }
        events
    }
    pub fn levels(&self) -> [f32; 5] {
        let Self::Running(session) = self else {
            return [0.; 5];
        };
        if !session.readiness.ready() {
            return [0.; 5];
        }
        let (input, output) = session.source.levels();
        bars(meter_level(
            (if session.failed_input { 0. } else { input }).max(if session.failed_output {
                0.
            } else {
                output
            }),
        ))
    }
}
impl Session {
    fn start(devices: &AudioSettings, model: ModelChoice, directory: &Path) -> Result<Self> {
        let base =
            PathBuf::from(std::env::var("HOME")?).join("Library/Application Support/team-up");
        if !base.join("models").join(model.model_file()).is_file() {
            bail!("The local {} model is not installed", model.short_name());
        }
        let runtime = if model == ModelChoice::Whisper {
            None
        } else {
            Some(Runtime::start_with_model(model)?)
        };
        let port = runtime.as_ref().map(|runtime| runtime.port).unwrap_or(0);
        let (tx, events) = mpsc::channel();
        let source = if let Some(path) = std::env::var_os("WORKLOG_AUDIO_FIXTURE") {
            let path = PathBuf::from(path);
            let stop = Arc::new(AtomicBool::new(false));
            let levels = Arc::new(local_audio::Levels::default());
            let worker_stop = stop.clone();
            let worker_levels = levels.clone();
            let worker = thread::spawn(move || {
                let result = (|| -> Result<()> {
                    local_audio::stream_wav_with_levels(
                        port,
                        &path,
                        model,
                        tx.clone(),
                        Some(worker_levels),
                    )?;
                    Ok(())
                })();
                worker_stop.store(true, Ordering::Relaxed);
                if let Err(error) = result {
                    let _ = tx.send(AudioEvent::Error(format!("{error:#}")));
                }
            });
            Source::Fixture {
                worker,
                stop,
                levels,
            }
        } else {
            std::fs::create_dir_all(directory).context("Cannot create the local audio folder")?;
            let selected = |device: &str| {
                if device == SYSTEM_DEFAULT {
                    String::new()
                } else {
                    device.to_owned()
                }
            };
            Source::Live(Capture::start(
                port,
                vec![
                    ("microphone".into(), false, selected(&devices.input)),
                    ("system audio".into(), true, selected(&devices.output)),
                ],
                CaptureOptions {
                    model,
                    audio_directory: Some(directory.into()),
                    retain_audio: true,
                    microphone_muted: false,
                },
                tx,
            ))
        };
        let readiness = Readiness::new(if matches!(source, Source::Fixture { .. }) {
            &["fixture"]
        } else {
            &["microphone", "system audio"]
        });
        Ok(Self {
            source,
            events,
            _runtime: runtime,
            readiness,
            failed_input: false,
            failed_output: false,
        })
    }
}
fn meter_level(rms: f32) -> f32 {
    if !rms.is_finite() || rms < 0.006 {
        0.
    } else {
        ((20. * rms.log10() + 44.) / 38.).clamp(0., 1.)
    }
}
fn bars(level: f32) -> [f32; 5] {
    [0.55, 0.8, 1., 0.8, 0.55].map(|weight| weight * level)
}
pub fn icon_pixels(levels: [f32; 5], recording: bool) -> Vec<u8> {
    let mut pixels = vec![0; 24 * 24 * 4];
    let color = if recording {
        [230, 107, 114, 255]
    } else {
        [150, 155, 164, 255]
    };
    for (i, level) in levels.into_iter().enumerate() {
        let height = 3 + (level.clamp(0., 1.) * 17.) as usize;
        for y in (24 - height) / 2..(24 + height) / 2 {
            for x in 2 + i * 4..5 + i * 4 {
                pixels[(y * 24 + x) * 4..(y * 24 + x + 1) * 4].copy_from_slice(&color);
            }
        }
    }
    pixels
}

/// A short, original two-tone cue; no external sound assets or player UI.
pub fn on_air_wave() -> Vec<u8> {
    let rate = 44_100_u32;
    let count = (rate as f32 * 0.26) as usize;
    let mut pcm = Vec::with_capacity(count * 2);
    for index in 0..count {
        let time = index as f32 / rate as f32;
        let (local, frequency) = if time < 0.12 {
            (time, 660.)
        } else {
            (time - 0.12, 880.)
        };
        let duration = if time < 0.12 { 0.12 } else { 0.14 };
        let envelope = (local / 0.008).min(1.) * ((duration - local) / 0.035).clamp(0., 1.);
        let sample =
            (0.22 * envelope * (std::f32::consts::TAU * frequency * local).sin() * i16::MAX as f32)
                as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    let mut wave = Vec::with_capacity(44 + pcm.len());
    wave.extend_from_slice(b"RIFF");
    wave.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wave.extend_from_slice(b"WAVEfmt ");
    wave.extend_from_slice(&16_u32.to_le_bytes());
    wave.extend_from_slice(&1_u16.to_le_bytes());
    wave.extend_from_slice(&1_u16.to_le_bytes());
    wave.extend_from_slice(&rate.to_le_bytes());
    wave.extend_from_slice(&(rate * 2).to_le_bytes());
    wave.extend_from_slice(&2_u16.to_le_bytes());
    wave.extend_from_slice(&16_u16.to_le_bytes());
    wave.extend_from_slice(b"data");
    wave.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wave.extend(pcm);
    wave
}
pub fn play_on_air() {
    if std::env::var_os("WORKLOG_AUDIO_FIXTURE").is_some()
        || std::env::var_os("WORKLOG_MUTE_CUES").is_some()
    {
        return;
    }
    thread::spawn(|| {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let play = || -> Result<()> {
            let mut file = tempfile::Builder::new().suffix(".wav").tempfile()?;
            file.write_all(&on_air_wave())?;
            Command::new("/usr/bin/afplay")
                .args(["-v", "0.35"])
                .arg(file.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            Ok(())
        };
        let _ = play();
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn on_air_waits_for_every_source_and_announces_only_once() {
        let mut gate = Readiness::new(&["microphone", "system audio"]);
        assert!(!gate.take_cue());
        gate.source_ready("microphone");
        gate.source_ready("microphone");
        assert!(!gate.ready());
        assert!(!gate.take_cue());
        gate.source_ready("system audio");
        assert!(gate.ready());
        assert!(gate.take_cue());
        assert!(!gate.take_cue());
        let mut failed = Readiness::new(&["microphone", "system audio"]);
        failed.source_ready("microphone");
        failed.failed = true;
        failed.source_ready("system audio");
        assert!(!failed.ready());
        assert!(!failed.take_cue());
    }
    #[test]
    fn on_air_cue_is_a_short_valid_quiet_pcm_wave() -> Result<()> {
        let mut wave = hound::WavReader::new(std::io::Cursor::new(on_air_wave()))?;
        assert_eq!(wave.spec().sample_rate, 44_100);
        assert_eq!(wave.spec().channels, 1);
        let samples = wave
            .samples::<i16>()
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert!((11_000..12_000).contains(&samples.len()));
        assert_eq!(samples[0], 0);
        assert!(samples.iter().any(|sample| *sample > 1000));
        assert!(samples.iter().all(|sample| sample.unsigned_abs() < 8000));
        Ok(())
    }
    #[test]
    fn meter_depends_on_audio_not_time() {
        assert_eq!(meter_level(0.), 0.);
        assert_eq!(meter_level(0.001), 0.);
        assert_eq!(meter_level(f32::NAN), 0.);
        let quiet = bars(meter_level(0.03));
        let loud = bars(meter_level(0.3));
        assert_ne!(quiet, loud);
        assert_ne!(icon_pixels(quiet, true), icon_pixels(loud, true));
        assert_eq!(bars(meter_level(0.03)), quiet);
        assert_eq!(Recording::default().levels(), [0.; 5]);
    }
    #[test]
    fn spoken_fixture_has_variable_levels() -> Result<()> {
        let mut reader = hound::WavReader::new(std::io::Cursor::new(include_bytes!(
            "../../../fixtures/asr/spoken.wav"
        )))?;
        let samples = reader
            .samples::<i16>()
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let levels = samples
            .chunks(512)
            .map(|chunk| {
                meter_level(
                    (chunk
                        .iter()
                        .map(|s| (f32::from(*s) / 32768.).powi(2))
                        .sum::<f32>()
                        / chunk.len() as f32)
                        .sqrt(),
                )
            })
            .collect::<Vec<_>>();
        assert!(levels.iter().any(|level| *level > 0.));
        assert!(levels.windows(2).any(|pair| pair[0] != pair[1]));
        Ok(())
    }
}

#[cfg(test)]
mod capture_problem_tests {
    use super::*;
    #[test]
    fn source_errors_have_actionable_help_and_preserve_original_details() {
        for (prefix, permission) in [
            ("system audio:", "Screen & System Audio Recording"),
            ("microphone:", "Microphone access"),
        ] {
            let raw = format!("{prefix} native device failed: \nfull diagnostic");
            let problem = CaptureProblem::from_error(&raw);
            assert_eq!(problem.details, raw);
            assert!(problem.help.contains(permission));
        }
    }
}
