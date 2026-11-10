use crate::{echo, model::Segment};
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
    Parakeet,
}
impl ModelChoice {
    pub const ALL: [Self; 3] = [Self::Nemotron, Self::Whisper, Self::Parakeet];
    pub fn short_name(self) -> &'static str {
        match self {
            Self::Nemotron => "Nemotron",
            Self::Whisper => "Whisper",
            Self::Parakeet => "Parakeet",
        }
    }
    pub fn model_file(self) -> &'static str {
        match self {
            Self::Nemotron => "nemotron-3.5-asr-streaming-0.6b.q8_0.gguf",
            Self::Whisper => "ggml-large-v3-turbo-q5_0.bin",
            Self::Parakeet => "parakeet-tdt-0.6b-v3.q8_0.gguf",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Nemotron => "Nemotron 3.5 Streaming · Q8_0",
            Self::Whisper => "Whisper large-v3-turbo · Q5_0",
            Self::Parakeet => "Parakeet TDT v3 · Q8_0",
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
    SourceReady {
        source: String,
    },
    Transcript(Segment),
    Partial {
        source: String,
        start: chrono::DateTime<Utc>,
        delta: String,
    },
    Flushed {
        generation: u64,
        source: String,
    },
    Error(String),
    Ready,
}
pub struct Runtime {
    child: Child,
    pub model: ModelChoice,
    pub port: u16,
}
impl Runtime {
    pub fn start() -> Result<Self> {
        Self::start_with_model(ModelChoice::Nemotron)
    }
    pub fn start_with_model(choice: ModelChoice) -> Result<Self> {
        if choice == ModelChoice::Whisper {
            bail!("Whisper uses its own local runtime");
        }
        let home = std::env::var("HOME")?;
        let base = PathBuf::from(home).join("Library/Application Support/team-up");
        let executable = base.join("runtime/nemo-speech-0.3.0-macos-aarch64-metal/bin/nemo-speech");
        let model = base.join("models").join(choice.model_file());
        if !model.is_file() {
            bail!(
                "The local {} model is not installed: {}",
                choice.short_name(),
                model.display()
            );
        }
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
            // Keep the server's stdin open independently of the GUI launcher.
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| {
                format!(
                    "Could not start the installed {} runtime",
                    choice.short_name()
                )
            })?;
        Ok(Self {
            child,
            port,
            model: choice,
        })
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
    input_updated: AtomicU64,
    output_updated: AtomicU64,
}
impl Levels {
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
    pub fn update(&self, output: bool, samples: &[f32]) {
        let energy = samples
            .iter()
            .map(|sample| {
                if sample.is_finite() {
                    sample.clamp(-1., 1.).powi(2)
                } else {
                    0.
                }
            })
            .sum::<f32>();
        let level = (energy / samples.len().max(1) as f32).sqrt();
        let (value, updated) = if output {
            (&self.output, &self.output_updated)
        } else {
            (&self.input, &self.input_updated)
        };
        value.store(level.to_bits(), Ordering::Relaxed);
        updated.store(Self::now_ms(), Ordering::Release);
    }
    pub fn values(&self) -> (f32, f32) {
        let now = Self::now_ms();
        let read = |value: &AtomicU32, updated: &AtomicU64| {
            if now.saturating_sub(updated.load(Ordering::Acquire)) > 300 {
                0.
            } else {
                f32::from_bits(value.load(Ordering::Relaxed))
            }
        };
        (
            read(&self.input, &self.input_updated),
            read(&self.output, &self.output_updated),
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
struct DeviceReadiness {
    ready: bool,
    deadline: Instant,
}
impl DeviceReadiness {
    fn new(output: bool, now: Instant) -> Self {
        Self {
            ready: output,
            deadline: now + Duration::from_secs(10),
        }
    }
    fn timed_out(&self, now: Instant) -> bool {
        !self.ready && now >= self.deadline
    }
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
        let echo_reference = (selections.iter().any(|(_, output, _)| *output)
            && selections.iter().any(|(_, output, _)| !*output))
        .then(|| Arc::new(echo::Reference::default()));
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
                let echo_reference = echo_reference.clone();
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
                        echo_reference,
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
                loop {
                    if Instant::now() >= until {
                        bail!("Speech runtime did not confirm transcription readiness");
                    }
                    match socket.read() {
                        Ok(Message::Text(text)) => {
                            let event: Value = serde_json::from_str(&text)?;
                            match event["type"].as_str() {
                                Some("session.updated") => return Ok(socket),
                                Some("error") => bail!(
                                    "Speech runtime: {}",
                                    event["message"]
                                        .as_str()
                                        .or_else(|| event["error"]["message"].as_str())
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| event.to_string())
                                ),
                                _ => {}
                            }
                        }
                        Ok(Message::Close(_)) => {
                            bail!("Speech runtime closed during initialization")
                        }
                        Err(tungstenite::Error::Io(error))
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Err(error) => return Err(error.into()),
                        _ => {}
                    }
                }
            }
            Err(error) if Instant::now() >= until => return Err(error.into()),
            Err(_) => thread::sleep(Duration::from_millis(200)),
        }
    }
}

// The callback never waits for inference. This sample budget keeps retained
// in-memory audio bounded while the recognizer finalizes a chunk.
struct CapturedAudio {
    samples: Vec<f32>,
    captured: Instant,
}
fn capture_instant(info: &cpal::InputCallbackInfo) -> Instant {
    let time = info.timestamp();
    let now = Instant::now();
    now.checked_sub(time.callback.duration_since(time.capture))
        .unwrap_or(now)
}
fn enqueue_audio(
    sender: &Sender<Result<CapturedAudio, String>>,
    data: Vec<f32>,
    captured: Instant,
    queued: &AtomicUsize,
    budget: usize,
    overflow: &AtomicBool,
) {
    let count = data.len();
    let previous = queued.fetch_add(count, Ordering::Relaxed);
    if previous.saturating_add(count) > budget
        || sender
            .send(Ok(CapturedAudio {
                samples: data,
                captured,
            }))
            .is_err()
    {
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
    retaining: &Arc<AtomicBool>,
    levels: &Arc<Levels>,
    flush_requested: &AtomicU64,
    echo_reference: Option<Arc<echo::Reference>>,
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
    let (sender, receiver) = mpsc::channel::<Result<CapturedAudio, String>>();
    let queued_samples = Arc::new(AtomicUsize::new(0));
    let callback_queued = queued_samples.clone();
    let sample_budget = rate as usize * channels * 35;
    let error_sender = sender.clone();
    let on_error = move |error: cpal::Error| {
        let _ = error_sender.send(Err(error.to_string()));
    };
    let overflow = Arc::new(AtomicBool::new(false));
    let callback_overflow = overflow.clone();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config.into(),
            move |data: &[f32], info| {
                let captured = capture_instant(info);
                let data = data.to_vec();
                enqueue_audio(
                    &sender,
                    data,
                    captured,
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
            move |data: &[i16], info| {
                let captured = capture_instant(info);
                let data = data.iter().map(|s| f32::from(*s) / 32768.0).collect();
                enqueue_audio(
                    &sender,
                    data,
                    captured,
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
    let (processed_tx, processed_rx) = mpsc::channel::<Result<Vec<u8>, String>>();
    let processed_queued = Arc::new(AtomicUsize::new(0));
    let preprocessor = {
        let queued = queued_samples.clone();
        let processed_queued = processed_queued.clone();
        let overflow = overflow.clone();
        let muted = muted.clone();
        let retaining = retaining.clone();
        let levels = levels.clone();
        let directory = options.audio_directory.clone();
        let source = source.to_owned();
        thread::spawn(move || {
            let result = preprocess_audio(
                receiver,
                &processed_tx,
                &queued,
                &processed_queued,
                &overflow,
                rate,
                channels,
                output,
                &muted,
                &retaining,
                &levels,
                echo_reference,
                directory,
                &source,
            );
            if let Err(error) = result {
                let _ = processed_tx.send(Err(format!("{error:#}")));
            }
        })
    };
    let receiver = processed_rx;
    stream.play()?;
    let mut readiness = DeviceReadiness::new(output, Instant::now());
    // Output loopback can remain silent (and deliver no callbacks) until sound
    // plays. Successful stream.play + ASR acknowledgement means it is armed.
    if readiness.ready {
        events.send(AudioEvent::SourceReady {
            source: source.into(),
        })?;
        events.send(AudioEvent::Ready)?;
    }
    let mut last_end = 0.0;
    let mut flushed_generation = 0;
    while !stop.load(Ordering::Relaxed) {
        if readiness.timed_out(Instant::now()) {
            bail!("No samples arrived from the selected microphone after 10 seconds");
        }
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
            let bytes = data.map_err(anyhow::Error::msg)?;
            processed_queued.fetch_sub(bytes.len(), Ordering::Relaxed);
            if !bytes.is_empty() {
                if !readiness.ready {
                    events.send(AudioEvent::SourceReady {
                        source: source.into(),
                    })?;
                    events.send(AudioEvent::Ready)?;
                    readiness.ready = true;
                }
                recognizer.push(bytes, events, source, started)?;
            }
        } else if preprocessor.is_finished() {
            bail!("Audio preprocessing stopped before capture ended");
        }
        recognizer.poll(events, source, started, &mut last_end)?;
    }
    drop(stream);
    // The capture callback is now closed. Drain preprocessing and ASR queues
    // before finalizing, including the final partial 10 ms echo frame.
    for data in receiver {
        let bytes = data.map_err(anyhow::Error::msg)?;
        processed_queued.fetch_sub(bytes.len(), Ordering::Relaxed);
        recognizer.push(bytes, events, source, started)?;
    }
    preprocessor
        .join()
        .map_err(|_| anyhow::anyhow!("Audio preprocessing panicked"))?;
    recognizer.finish(events, source, started, &mut last_end)
}

#[allow(clippy::too_many_arguments)]
fn preprocess_audio(
    receiver: mpsc::Receiver<Result<CapturedAudio, String>>,
    sender: &Sender<Result<Vec<u8>, String>>,
    queued: &AtomicUsize,
    processed_queued: &AtomicUsize,
    overflow: &AtomicBool,
    rate: u32,
    channels: usize,
    output: bool,
    muted: &AtomicBool,
    retaining: &AtomicBool,
    levels: &Levels,
    reference: Option<Arc<echo::Reference>>,
    directory: Option<PathBuf>,
    source: &str,
) -> Result<()> {
    let mut converter = Resampler::new(rate, channels);
    let mut retained = AudioRetention {
        directory,
        source: source.into(),
        writer: None,
    };
    let mut cleaner = (!output && reference.is_some()).then(echo::Cleaner::default);
    let mut next_time: Option<i64> = None;
    let send = |samples: &[f32]| -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        levels.update(output, samples);
        let bytes: Vec<u8> = samples
            .iter()
            .flat_map(|sample| ((sample.clamp(-1., 1.) * 32767.) as i16).to_le_bytes())
            .collect();
        let count = bytes.len();
        let previous = processed_queued.fetch_add(count, Ordering::Relaxed);
        if previous.saturating_add(count) > echo::RATE * 2 * 35 || sender.send(Ok(bytes)).is_err() {
            processed_queued.fetch_sub(count, Ordering::Relaxed);
            overflow.store(true, Ordering::Relaxed);
            bail!("Processed audio buffer overflow; capture stopped to avoid silent data loss");
        }
        Ok(())
    };
    for packet in receiver {
        let mut packet = packet.map_err(anyhow::Error::msg)?;
        queued.fetch_sub(packet.samples.len(), Ordering::Relaxed);
        let captured = packet.captured;
        if !output && muted.load(Ordering::Relaxed) {
            packet.samples.fill(0.);
        }
        let bytes = converter.convert(&packet.samples);
        // Retention deliberately precedes echo processing. Original source
        // audio remains recoverable even when recognition/filtering changes.
        retained.write(&bytes, retaining.load(Ordering::Relaxed))?;
        let raw: Vec<f32> = bytes
            .chunks_exact(2)
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.)
            .collect();
        let measured = reference
            .as_ref()
            .map(|reference| reference.position(captured))
            .unwrap_or(0);
        let time = next_time
            .filter(|next| (measured - next).abs() < echo::RATE as i64 / 10)
            .unwrap_or(measured);
        next_time = Some(time + raw.len() as i64);
        if let Some(reference) = &reference {
            if output {
                reference.push(time, raw.clone())?;
            }
            if let Some(cleaner) = &mut cleaner {
                send(&cleaner.push(time, &raw, reference)?)?;
                continue;
            }
        }
        send(&raw)?;
    }
    if let Some(cleaner) = &mut cleaner {
        send(&cleaner.finish(reference.as_deref().context("Missing echo reference")?)?)?;
    }
    retained.finish()
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
                    bail!(
                        "Speech runtime: {}",
                        value["message"]
                            .as_str()
                            .or_else(|| value["error"]["message"].as_str())
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    );
                }
                if kind == "conversation.item.input_audio_transcription.delta" {
                    if let Some(delta) = value["delta"].as_str().filter(|text| !text.is_empty()) {
                        events.send(AudioEvent::Partial {
                            source: source.into(),
                            start: started
                                + ChronoDuration::milliseconds((*last_end * 1000.) as i64),
                            delta: delta.into(),
                        })?;
                    }
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
    stream_wav_with_levels(port, path, model, events, None)
}
pub fn stream_wav_with_levels(
    port: u16,
    path: &Path,
    model: ModelChoice,
    events: Sender<AudioEvent>,
    levels: Option<Arc<Levels>>,
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
    events.send(AudioEvent::Ready)?;
    for (index, chunk) in samples.chunks(frames).enumerate() {
        let data: Vec<_> = chunk.iter().map(|s| f32::from(*s) / 32768.0).collect();
        if let Some(levels) = &levels {
            levels.update(false, &data);
        }
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
    Buffered {
        bytes: Vec<u8>,
        offset: f64,
        model: ModelChoice,
        port: u16,
    },
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
            ModelChoice::Whisper | ModelChoice::Parakeet => {
                if choice == ModelChoice::Parakeet {
                    wait_for_http_ready(port)?;
                }
                Self::Buffered {
                    bytes: vec![],
                    offset: 0.,
                    model: choice,
                    port,
                }
            }
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
            Self::Buffered {
                bytes: buffer,
                offset,
                model,
                port,
            } => {
                buffer.extend(bytes);
                if buffer.len() >= 16000 * 2 * 8 {
                    buffered_chunk(*model, *port, buffer, offset, events, source, started)?;
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
            Self::Buffered {
                bytes,
                offset,
                model,
                port,
            } => buffered_chunk(*model, *port, bytes, offset, events, source, started),
        }
    }
}

fn wait_for_http_ready(port: u16) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(90);
    loop {
        let response = Command::new("/usr/bin/curl")
            .args([
                "--silent",
                "--show-error",
                "--fail-with-body",
                "--max-time",
                "2",
            ])
            .arg(format!("http://127.0.0.1:{port}/ready"))
            .output()?;
        if response.status.success()
            && serde_json::from_slice::<Value>(&response.stdout)
                .ok()
                .is_some_and(|value| value["ready"] == true || value["status"] == "ok")
        {
            return Ok(());
        }
        if Instant::now() >= until {
            bail!(
                "Local Parakeet runtime did not become ready: {}",
                String::from_utf8_lossy(&response.stderr)
            );
        }
        thread::sleep(Duration::from_millis(200));
    }
}
fn buffered_chunk(
    model_choice: ModelChoice,
    port: u16,
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
    let text = if model_choice == ModelChoice::Parakeet {
        let output = Command::new("/usr/bin/curl")
            .args([
                "--silent",
                "--show-error",
                "--fail-with-body",
                "--connect-timeout",
                "2",
                "--max-time",
                "45",
            ])
            .arg(format!("http://127.0.0.1:{port}/v1/audio/transcriptions"))
            .args(["-F", "response_format=json", "-F"])
            .arg(format!("file=@{}", input.display()))
            .output()?;
        if !output.status.success() {
            bail!(
                "Parakeet transcription failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let response: Value = serde_json::from_slice(&output.stdout)
            .context("Invalid Parakeet transcription response")?;
        response["text"]
            .as_str()
            .context("Parakeet returned no transcript field")?
            .trim()
            .to_owned()
    } else {
        let home = std::env::var("HOME")?;
        let model = PathBuf::from(home)
            .join("Library/Application Support/team-up/models")
            .join(ModelChoice::Whisper.model_file());
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
        String::from_utf8(output.stdout)?.trim().to_owned()
    };
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
    fn transcription_connect_waits_for_session_confirmation() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let (seen_tx, seen_rx) = mpsc::channel();
        let (allow_tx, allow_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let server = thread::spawn(move || -> Result<()> {
            let (stream, _) = listener.accept()?;
            let mut socket = tungstenite::accept(stream)?;
            let _ = socket.read()?;
            socket.send(Message::Text(
                json!({"type":"session.created"}).to_string().into(),
            ))?;
            seen_tx.send(())?;
            allow_rx.recv()?;
            socket.send(Message::Text(
                json!({"type":"session.updated"}).to_string().into(),
            ))?;
            Ok(())
        });
        let client = thread::spawn(move || {
            let _ = ready_tx.send(connect(port).map(|_| ()));
        });
        seen_rx.recv_timeout(Duration::from_secs(2))?;
        assert!(matches!(
            ready_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        allow_tx.send(())?;
        ready_rx.recv_timeout(Duration::from_secs(2))??;
        server
            .join()
            .map_err(|_| anyhow::anyhow!("Server panicked"))??;
        client
            .join()
            .map_err(|_| anyhow::anyhow!("Client panicked"))?;
        Ok(())
    }

    #[test]
    fn realtime_partials_are_forwarded_before_the_authoritative_final() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = thread::spawn(move || -> Result<()> {
            let (stream, _) = listener.accept()?;
            let mut socket = tungstenite::accept(stream)?;
            let _ = socket.read()?;
            socket.send(Message::Text(
                json!({"type":"session.updated"}).to_string().into(),
            ))?;
            for value in [
                json!({"type":"conversation.item.input_audio_transcription.delta","delta":"hello"}),
                json!({"type":"conversation.item.input_audio_transcription.delta","delta":" world"}),
                json!({"type":"conversation.item.input_audio_transcription.completed","transcript":"Hello, world.","words":[{"start":0.,"end":1.2}]}),
                json!({"type":"input_audio_buffer.committed"}),
            ] {
                socket.send(Message::Text(value.to_string().into()))?;
            }
            Ok(())
        });
        let mut socket = connect(port)?;
        let (tx, rx) = mpsc::channel();
        let mut end = 0.;
        while !drain(&mut socket, &tx, "microphone", Utc::now(), &mut end)? {}
        let events: Vec<_> = rx.try_iter().collect();
        assert!(matches!(&events[0],AudioEvent::Partial {delta,..} if delta=="hello"));
        assert!(matches!(&events[1],AudioEvent::Partial {delta,..} if delta==" world"));
        assert!(
            matches!(&events[2],AudioEvent::Transcript(segment) if segment.text=="Hello, world.")
        );
        assert_eq!(end, 1.2);
        server
            .join()
            .map_err(|_| anyhow::anyhow!("WebSocket server panicked"))??;
        Ok(())
    }

    #[test]
    fn levels_are_live_even_while_inference_is_busy_and_expire_on_disconnect() {
        let levels = Levels::default();
        levels.update(false, &[0.1; 32]);
        levels.update(true, &[0.2; 32]);
        let (mic, system) = levels.values();
        assert!((mic - 0.1).abs() < 0.001 && (system - 0.2).abs() < 0.001);
        levels.update(false, &[0.; 32]);
        assert_eq!(levels.values().0, 0.);
        levels.output_updated.store(0, Ordering::Release);
        assert_eq!(levels.values().1, 0.);
    }
    #[test]
    fn capture_buffers_a_full_finalization_delay_for_different_device_buffers() {
        for callback_samples in [128, 512, 2048] {
            let (tx, rx) = mpsc::channel();
            let queued = AtomicUsize::new(0);
            let overflow = AtomicBool::new(false);
            let budget = 48_000 * 35;
            for _ in 0..(48_000 * 30 / callback_samples) {
                enqueue_audio(
                    &tx,
                    vec![0.0; callback_samples],
                    Instant::now(),
                    &queued,
                    budget,
                    &overflow,
                );
            }
            assert!(!overflow.load(Ordering::Relaxed));
            let buffered = queued.load(Ordering::Relaxed);
            let received: usize = rx
                .try_iter()
                .filter_map(Result::ok)
                .map(|data| data.samples.len())
                .sum();
            assert_eq!(received, buffered);
            // A stalled recognizer still cannot consume unbounded memory.
            enqueue_audio(
                &tx,
                vec![0.0; budget],
                Instant::now(),
                &queued,
                budget,
                &overflow,
            );
            assert!(overflow.load(Ordering::Relaxed));
            assert!(rx.try_recv().is_err());
            assert_eq!(queued.load(Ordering::Relaxed), buffered);
        }
    }
}

#[cfg(test)]
mod preprocessing_tests {
    use super::*;
    #[test]
    fn raw_audio_is_retained_while_clean_audio_and_levels_feed_the_asr_queue() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let reference = Arc::new(echo::Reference::default());
        let input: Vec<f32> = (0..3217)
            .map(|i| {
                if i == 3100 {
                    0.7
                } else {
                    0.1 * (i as f32 / 11.).sin()
                }
            })
            .collect();
        let (raw_tx, raw_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        raw_tx.send(Ok(CapturedAudio {
            samples: input.clone(),
            captured: Instant::now(),
        }))?;
        drop(raw_tx);
        let queued = AtomicUsize::new(input.len());
        let processed = AtomicUsize::new(0);
        let overflow = AtomicBool::new(false);
        let levels = Levels::default();
        preprocess_audio(
            raw_rx,
            &tx,
            &queued,
            &processed,
            &overflow,
            16_000,
            1,
            false,
            &AtomicBool::new(false),
            &AtomicBool::new(true),
            &levels,
            Some(reference),
            Some(dir.path().to_owned()),
            "microphone",
        )?;
        drop(tx);
        let clean = rx
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(anyhow::Error::msg)?
            .concat();
        assert!(!overflow.load(Ordering::Relaxed));
        assert_eq!(queued.load(Ordering::Relaxed), 0);
        assert_eq!(clean.len() / 2, input.len() + 128);
        let files = std::fs::read_dir(dir.path())?.collect::<std::io::Result<Vec<_>>>()?;
        assert_eq!(files.len(), 1);
        let retained = hound::WavReader::open(files[0].path())?
            .samples::<i16>()
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(
            retained,
            input
                .iter()
                .map(|s| (s * 32767.) as i16)
                .collect::<Vec<_>>()
        );
        assert!(
            levels.values().0 > 0.,
            "Meter did not receive cleaned microphone audio"
        );
        Ok(())
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::*;
    #[test]
    fn all_three_engines_have_distinct_models_and_persisted_names() {
        assert_eq!(
            ModelChoice::ALL.map(|model| model.short_name()),
            ["Nemotron", "Whisper", "Parakeet"]
        );
        for model in ModelChoice::ALL {
            assert_eq!(
                serde_json::from_str::<ModelChoice>(&serde_json::to_string(&model).unwrap())
                    .unwrap(),
                model
            );
        }
        assert!(ModelChoice::Parakeet.model_file().contains("parakeet-tdt"));
        assert_ne!(
            ModelChoice::Parakeet.model_file(),
            ModelChoice::Nemotron.model_file()
        );
    }
    #[test]
    fn silent_output_is_ready_but_missing_microphone_samples_time_out() {
        let now = Instant::now();
        let output = DeviceReadiness::new(true, now);
        assert!(output.ready);
        assert!(!output.timed_out(now + Duration::from_secs(3600)));
        let mut mic = DeviceReadiness::new(false, now);
        assert!(!mic.timed_out(now + Duration::from_secs(9)));
        assert!(mic.timed_out(now + Duration::from_secs(10)));
        mic.ready = true;
        assert!(!mic.timed_out(now + Duration::from_secs(3600)));
    }
}
