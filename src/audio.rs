use crate::model::Segment;
use anyhow::{Context, Result, bail};
use chrono::{Duration as ChronoDuration, Utc};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde_json::{Value, json};
use std::{
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelChoice {
    #[default]
    Nemotron,
    Whisper,
}
impl ModelChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nemotron => "Nemotron 3.5 Streaming · Q8_0",
            Self::Whisper => "Whisper large-v3-turbo · Q5_0",
        }
    }
}
#[derive(Clone)]
pub struct CaptureOptions {
    pub model: ModelChoice,
    pub audio_directory: Option<PathBuf>,
    pub retain_audio: bool,
    pub microphone_muted: bool,
}
pub enum AudioEvent {
    Transcript(Segment),
    Flushed { generation: u64, source: String },
    Error(String),
    Ready,
}
pub struct Runtime {
    child: Child,
    pub port: u16,
}
impl Runtime {
    pub fn start() -> Result<Self> {
        let home = std::env::var("HOME")?;
        let base = PathBuf::from(home).join("Library/Application Support/team-up");
        let executable = base.join("runtime/nemo-speech-0.3.0-macos-aarch64-metal/bin/nemo-speech");
        let model = base.join("models/nemotron-3.5-asr-streaming-0.6b.q8_0.gguf");
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let child = Command::new(executable)
            .args([
                "serve",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--asr-model",
            ])
            .arg(model)
            .args(["--device", "metal", "--no-ui"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Could not start the installed Nemotron runtime")?;
        Ok(Self { child, port })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn devices(output: bool) -> Vec<String> {
    let host = cpal::default_host();
    let devices = if output {
        host.output_devices()
    } else {
        host.input_devices()
    };
    devices
        .map(|items| items.map(|d| d.to_string()).collect())
        .unwrap_or_default()
}

#[derive(Default)]
pub struct Levels {
    pub input: AtomicU32,
    pub output: AtomicU32,
}
impl Levels {
    pub fn values(&self) -> (f32, f32) {
        (
            f32::from_bits(self.input.load(Ordering::Relaxed)),
            f32::from_bits(self.output.load(Ordering::Relaxed)),
        )
    }
}
pub struct Capture {
    stop: Arc<AtomicBool>,
    pub muted: Arc<AtomicBool>,
    pub retaining: Arc<AtomicBool>,
    pub levels: Arc<Levels>,
    flush_requested: Arc<AtomicU64>,
    workers: Vec<thread::JoinHandle<()>>,
}
impl Capture {
    pub fn start(
        port: u16,
        selections: Vec<(String, bool, String)>,
        options: CaptureOptions,
        events: Sender<AudioEvent>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let muted = Arc::new(AtomicBool::new(options.microphone_muted));
        let retaining = Arc::new(AtomicBool::new(options.retain_audio));
        let levels = Arc::new(Levels::default());
        let flush_requested = Arc::new(AtomicU64::new(0));
        let workers = selections
            .into_iter()
            .map(|(name, output, selected)| {
                let stop = stop.clone();
                let events = events.clone();
                let options = options.clone();
                let muted = muted.clone();
                let retaining = retaining.clone();
                let levels = levels.clone();
                let flush_requested = flush_requested.clone();
                thread::spawn(move || {
                    if let Err(error) = capture_device(
                        port,
                        &name,
                        output,
                        &selected,
                        &options,
                        &stop,
                        &muted,
                        &retaining,
                        &levels,
                        &flush_requested,
                        &events,
                    ) {
                        let _ = events.send(AudioEvent::Error(format!("{name}: {error:#}")));
                    }
                })
            })
            .collect();
        Self {
            stop,
            muted,
            retaining,
            levels,
            flush_requested,
            workers,
        }
    }
    pub fn flush(&self) -> u64 {
        self.flush_requested.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn finished(&self) -> bool {
        self.workers.iter().all(thread::JoinHandle::is_finished)
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn connect(port: u16) -> Result<WebSocket<MaybeTlsStream<TcpStream>>> {
    let until = Instant::now() + Duration::from_secs(90);
    loop {
        match tungstenite::connect(format!(
            "ws://127.0.0.1:{port}/v1/audio/transcriptions/realtime"
        )) {
            Ok((mut socket, _)) => {
                if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
                    stream.set_read_timeout(Some(Duration::from_millis(10)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                }
                socket.send(Message::Text(json!({"type":"session.update", "session":{"sample_rate":16000,"word_timestamps":true,"endpointing_ms":600}}).to_string().into()))?;
                return Ok(socket);
            }
            Err(error) if Instant::now() >= until => return Err(error.into()),
            Err(_) => thread::sleep(Duration::from_millis(200)),
        }
    }
}

// The callback never waits for inference. This sample budget keeps retained
// in-memory audio bounded while the recognizer finalizes a chunk.
fn enqueue_audio(
    sender: &Sender<Result<Vec<f32>, String>>,
    data: Vec<f32>,
    queued: &AtomicUsize,
    budget: usize,
    overflow: &AtomicBool,
) {
    let count = data.len();
    let previous = queued.fetch_add(count, Ordering::Relaxed);
    if previous.saturating_add(count) > budget || sender.send(Ok(data)).is_err() {
        queued.fetch_sub(count, Ordering::Relaxed);
        overflow.store(true, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_arguments)]
fn capture_device(
    port: u16,
    source: &str,
    output: bool,
    selected: &str,
    options: &CaptureOptions,
    stop: &AtomicBool,
    muted: &Arc<AtomicBool>,
    retaining: &AtomicBool,
    levels: &Levels,
    flush_requested: &AtomicU64,
    events: &Sender<AudioEvent>,
) -> Result<()> {
    let mut recognizer = Recognizer::new(port, options.model)?;
    let host = cpal::default_host();
    let mut devices = if output {
        host.output_devices()?
    } else {
        host.input_devices()?
    };
    let device = if selected.is_empty() {
        if output {
            host.default_output_device()
        } else {
            host.default_input_device()
        }
    } else {
        devices.find(|d| d.to_string() == selected)
    }
    .context("Selected audio device is unavailable")?;
    let config = if output {
        device.default_output_config()?
    } else {
        device.default_input_config()?
    };
    let channels = usize::from(config.channels());
    let rate = config.sample_rate();
    // Finalization can take up to 30 seconds. Bound by audio duration rather
    // than callback count: device buffer sizes vary, and 64 callbacks could
    // overflow during a normal finalization and stop an all-day recording.
    let (sender, receiver) = mpsc::channel::<Result<Vec<f32>, String>>();
    let queued_samples = Arc::new(AtomicUsize::new(0));
    let callback_queued = queued_samples.clone();
    let sample_budget = rate as usize * channels * 35;
    let error_sender = sender.clone();
    let on_error = move |error: cpal::Error| {
        let _ = error_sender.send(Err(error.to_string()));
    };
    let overflow = Arc::new(AtomicBool::new(false));
    let callback_overflow = overflow.clone();
    let callback_muted = muted.clone();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config.into(),
            move |data: &[f32], _| {
                let data = if !output && callback_muted.load(Ordering::Relaxed) {
                    vec![0.0; data.len()]
                } else {
                    data.to_vec()
                };
                enqueue_audio(
                    &sender,
                    data,
                    &callback_queued,
                    sample_budget,
                    &callback_overflow,
                );
            },
            on_error,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            config.into(),
            move |data: &[i16], _| {
                let data = if !output && callback_muted.load(Ordering::Relaxed) {
                    vec![0.0; data.len()]
                } else {
                    data.iter().map(|s| f32::from(*s) / 32768.0).collect()
                };
                enqueue_audio(
                    &sender,
                    data,
                    &callback_queued,
                    sample_budget,
                    &callback_overflow,
                );
            },
            on_error,
            None,
        )?,
        format => bail!("Unsupported device sample format {format}"),
    };
    let started = Utc::now();
    let mut retained = AudioRetention {
        directory: options.audio_directory.clone(),
        source: source.into(),
        writer: None,
    };
    stream.play()?;
    events.send(AudioEvent::Ready)?;
    let mut converter = Resampler::new(rate, channels);
    let mut last_end = 0.0;
    let mut flushed_generation = 0;
    while !stop.load(Ordering::Relaxed) {
        let requested = flush_requested.load(Ordering::SeqCst);
        if requested > flushed_generation {
            recognizer.checkpoint(events, source, started, &mut last_end)?;
            events.send(AudioEvent::Flushed {
                generation: requested,
                source: source.into(),
            })?;
            flushed_generation = requested;
        }
        if overflow.load(Ordering::Relaxed) {
            bail!("Audio buffer overflow; recording stopped to avoid silent data loss");
        }
        if let Ok(data) = receiver.recv_timeout(Duration::from_millis(20)) {
            let mut data = data.map_err(anyhow::Error::msg)?;
            queued_samples.fetch_sub(data.len(), Ordering::Relaxed);
            if !output && muted.load(Ordering::Relaxed) {
                data.fill(0.0);
            }
            let level = (data.iter().map(|sample| sample * sample).sum::<f32>()
                / data.len().max(1) as f32)
                .sqrt();
            (if output {
                &levels.output
            } else {
                &levels.input
            })
            .store(level.to_bits(), Ordering::Relaxed);
            let bytes = converter.convert(&data);
            retained.write(&bytes, retaining.load(Ordering::Relaxed))?;
            if !bytes.is_empty() {
                recognizer.push(bytes, events, source, started)?;
            }
        }
        recognizer.poll(events, source, started, &mut last_end)?;
    }
    drop(stream);
    while let Ok(data) = receiver.try_recv() {
        let mut data = data.map_err(anyhow::Error::msg)?;
        queued_samples.fetch_sub(data.len(), Ordering::Relaxed);
        if !output && muted.load(Ordering::Relaxed) {
            data.fill(0.0);
        }
        let bytes = converter.convert(&data);
        retained.write(&bytes, retaining.load(Ordering::Relaxed))?;
        recognizer.push(bytes, events, source, started)?;
    }
    retained.finish()?;
    recognizer.finish(events, source, started, &mut last_end)
}

struct AudioRetention {
    directory: Option<PathBuf>,
    source: String,
    writer: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
}
impl AudioRetention {
    fn write(&mut self, bytes: &[u8], enabled: bool) -> Result<()> {
        if !enabled {
            return self.finish();
        }
        if self.writer.is_none()
            && let Some(directory) = &self.directory
        {
            std::fs::create_dir_all(directory)?;
            let name = format!(
                "{}-{}-{}.wav",
                Utc::now().format("%Y-%m-%d_%H-%M-%S"),
                self.source.replace(' ', "-"),
                Uuid::new_v4()
            );
            self.writer = Some(hound::WavWriter::create(
                directory.join(name),
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 16000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )?);
        }
        if let Some(writer) = &mut self.writer {
            for sample in bytes.as_chunks::<2>().0 {
                writer.write_sample(i16::from_le_bytes(*sample))?;
            }
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.take() {
            writer.finalize()?;
        }
        Ok(())
    }
}

pub struct Resampler {
    rate: u32,
    channels: usize,
    phase: u64,
}
impl Resampler {
    pub fn new(rate: u32, channels: usize) -> Self {
        Self {
            rate,
            channels,
            phase: 0,
        }
    }
    pub fn convert(&mut self, data: &[f32]) -> Vec<u8> {
        let mut result = Vec::new();
        for frame in data.chunks_exact(self.channels.max(1)) {
            self.phase += 16000;
            while self.phase >= u64::from(self.rate) {
                self.phase -= u64::from(self.rate);
                let mono = frame.iter().sum::<f32>() / self.channels.max(1) as f32;
                result.extend_from_slice(&((mono.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            }
        }
        result
    }
}
fn drain(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    events: &Sender<AudioEvent>,
    source: &str,
    started: chrono::DateTime<Utc>,
    last_end: &mut f64,
) -> Result<bool> {
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                let value: Value = serde_json::from_str(&text)?;
                let kind = value["type"].as_str().unwrap_or("");
                if kind == "error" {
                    bail!("Speech runtime: {}", value["message"]);
                }
                if kind.ends_with(".completed") {
                    let text = value["transcript"]
                        .as_str()
                        .or_else(|| value["text"].as_str())
                        .unwrap_or("")
                        .trim();
                    if !text.is_empty() {
                        let words = value["words"].as_array();
                        let start = words
                            .and_then(|w| w.first())
                            .and_then(|w| w["start"].as_f64())
                            .unwrap_or(*last_end);
                        let end = words
                            .and_then(|w| w.last())
                            .and_then(|w| w["end"].as_f64())
                            .unwrap_or_else(|| {
                                (Utc::now() - started).num_milliseconds() as f64 / 1000.0
                            });
                        *last_end = end;
                        events.send(AudioEvent::Transcript(Segment {
                            id: Uuid::new_v4(),
                            source: source.into(),
                            start: started + ChronoDuration::milliseconds((start * 1000.0) as i64),
                            end: started + ChronoDuration::milliseconds((end * 1000.0) as i64),
                            text: text.into(),
                        }))?;
                    }
                }
                if kind == "input_audio_buffer.committed" {
                    return Ok(true);
                }
            }
            Ok(Message::Close(_)) => return Ok(true),
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Feeds a real WAV, paced at wall-clock speed, through the same WebSocket and finalizer
/// as device capture. This exercises real local inference without opening a microphone.
pub fn stream_wav(port: u16, path: &Path, events: Sender<AudioEvent>) -> Result<()> {
    stream_wav_with_model(port, path, ModelChoice::Nemotron, events)
}
pub fn stream_wav_with_model(
    port: u16,
    path: &Path,
    model: ModelChoice,
    events: Sender<AudioEvent>,
) -> Result<()> {
    let mut recognizer = Recognizer::new(port, model)?;
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        bail!("E2E fixture must be PCM16 WAV");
    }
    let samples = reader
        .samples::<i16>()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut converter = Resampler::new(spec.sample_rate, usize::from(spec.channels));
    let started = Utc::now();
    let clock = Instant::now();
    let mut last_end = 0.0;
    let frames = (spec.sample_rate as usize / 10) * usize::from(spec.channels);
    for (index, chunk) in samples.chunks(frames).enumerate() {
        let data: Vec<_> = chunk.iter().map(|s| f32::from(*s) / 32768.0).collect();
        recognizer.push(converter.convert(&data), &events, "microphone", started)?;
        recognizer.poll(&events, "microphone", started, &mut last_end)?;
        if let Some(delay) =
            Duration::from_millis((index as u64 + 1) * 100).checked_sub(clock.elapsed())
        {
            thread::sleep(delay);
        }
    }
    recognizer.finish(&events, "microphone", started, &mut last_end)
}

struct LiveNemo {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    port: u16,
    offset: f64,
    frames: usize,
    silence: usize,
    voiced: bool,
    last_end: f64,
}
enum Recognizer {
    Nemotron(Box<LiveNemo>),
    Whisper { bytes: Vec<u8>, offset: f64 },
}
impl Recognizer {
    fn new(port: u16, choice: ModelChoice) -> Result<Self> {
        Ok(match choice {
            ModelChoice::Nemotron => Self::Nemotron(Box::new(LiveNemo {
                socket: connect(port)?,
                port,
                offset: 0.0,
                frames: 0,
                silence: 0,
                voiced: false,
                last_end: 0.0,
            })),
            ModelChoice::Whisper => Self::Whisper {
                bytes: vec![],
                offset: 0.0,
            },
        })
    }
    fn push(
        &mut self,
        bytes: Vec<u8>,
        events: &Sender<AudioEvent>,
        source: &str,
        started: chrono::DateTime<Utc>,
    ) -> Result<()> {
        let mut checkpoint = false;
        match self {
            Self::Nemotron(live) => {
                let frames = bytes.len() / 2;
                let energy = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|sample| f64::from(i16::from_le_bytes(*sample)).abs())
                    .sum::<f64>()
                    / frames.max(1) as f64;
                if energy > 180.0 {
                    live.voiced = true;
                    live.silence = 0;
                } else {
                    live.silence += frames;
                }
                live.frames += frames;
                live.socket.send(Message::Binary(bytes.into()))?;
                checkpoint = live.frames >= 16000 * 8 || (live.voiced && live.silence >= 12000);
            }
            Self::Whisper {
                bytes: buffer,
                offset,
            } => {
                buffer.extend(bytes);
                if buffer.len() >= 16000 * 2 * 8 {
                    whisper_chunk(buffer, offset, events, source, started)?;
                }
            }
        }
        if checkpoint {
            let mut last = 0.0;
            self.checkpoint(events, source, started, &mut last)?;
        }
        Ok(())
    }
    fn poll(
        &mut self,
        events: &Sender<AudioEvent>,
        source: &str,
        started: chrono::DateTime<Utc>,
        _last: &mut f64,
    ) -> Result<()> {
        if let Self::Nemotron(live) = self {
            let base = started + ChronoDuration::milliseconds((live.offset * 1000.0) as i64);
            drain(&mut live.socket, events, source, base, &mut live.last_end)?;
        }
        Ok(())
    }
    fn checkpoint(
        &mut self,
        events: &Sender<AudioEvent>,
        source: &str,
        started: chrono::DateTime<Utc>,
        last: &mut f64,
    ) -> Result<()> {
        self.finish(events, source, started, last)?;
        if let Self::Nemotron(live) = self {
            live.offset += live.frames as f64 / 16000.0;
            live.frames = 0;
            live.voiced = false;
            live.silence = 0;
            live.last_end = 0.0;
            live.socket = connect(live.port)?;
        }
        Ok(())
    }
    fn finish(
        &mut self,
        events: &Sender<AudioEvent>,
        source: &str,
        started: chrono::DateTime<Utc>,
        _last: &mut f64,
    ) -> Result<()> {
        match self {
            Self::Nemotron(live) => {
                if live.frames == 0 {
                    return Ok(());
                }
                live.socket.send(Message::Text(
                    json!({"type":"input_audio_buffer.commit"})
                        .to_string()
                        .into(),
                ))?;
                let until = Instant::now() + Duration::from_secs(30);
                let base = started + ChronoDuration::milliseconds((live.offset * 1000.0) as i64);
                while Instant::now() < until {
                    if drain(&mut live.socket, events, source, base, &mut live.last_end)? {
                        return Ok(());
                    }
                }
                bail!("Speech runtime did not acknowledge final audio");
            }
            Self::Whisper { bytes, offset } => {
                whisper_chunk(bytes, offset, events, source, started)
            }
        }
    }
}

fn whisper_chunk(
    bytes: &mut Vec<u8>,
    offset: &mut f64,
    events: &Sender<AudioEvent>,
    source: &str,
    started: chrono::DateTime<Utc>,
) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let temporary = tempfile::tempdir()?;
    let input = temporary.path().join("chunk.wav");
    let mut writer = hound::WavWriter::create(
        &input,
        hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for sample in bytes.as_chunks::<2>().0 {
        writer.write_sample(i16::from_le_bytes([sample[0], sample[1]]))?;
    }
    writer.finalize()?;
    let home = std::env::var("HOME")?;
    let model = PathBuf::from(home)
        .join("Library/Application Support/team-up/models/ggml-large-v3-turbo-q5_0.bin");
    let output = Command::new("/opt/homebrew/bin/whisper-cli")
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(input)
        .args(["-l", "auto", "-nt", "-np"])
        .output()?;
    if !output.status.success() {
        bail!(
            "Whisper transcription failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = String::from_utf8(output.stdout)?.trim().to_owned();
    let end = *offset + bytes.len() as f64 / 32000.0;
    if !text.is_empty() {
        events.send(AudioEvent::Transcript(Segment {
            id: Uuid::new_v4(),
            source: source.into(),
            start: started + ChronoDuration::milliseconds((*offset * 1000.0) as i64),
            end: started + ChronoDuration::milliseconds((end * 1000.0) as i64),
            text,
        }))?;
    }
    bytes.clear();
    *offset = end;
    Ok(())
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    #[test]
    fn capture_buffers_a_full_finalization_delay_for_different_device_buffers() {
        for callback_samples in [128, 512, 2048] {
            let (tx, rx) = mpsc::channel();
            let queued = AtomicUsize::new(0);
            let overflow = AtomicBool::new(false);
            let budget = 48_000 * 35;
            for _ in 0..(48_000 * 30 / callback_samples) {
                enqueue_audio(&tx, vec![0.0; callback_samples], &queued, budget, &overflow);
            }
            assert!(!overflow.load(Ordering::Relaxed));
            let buffered = queued.load(Ordering::Relaxed);
            let received: usize = rx
                .try_iter()
                .filter_map(Result::ok)
                .map(|data| data.len())
                .sum();
            assert_eq!(received, buffered);
            // A stalled recognizer still cannot consume unbounded memory.
            enqueue_audio(&tx, vec![0.0; budget], &queued, budget, &overflow);
            assert!(overflow.load(Ordering::Relaxed));
            assert!(rx.try_recv().is_err());
            assert_eq!(queued.load(Ordering::Relaxed), buffered);
        }
    }
}
